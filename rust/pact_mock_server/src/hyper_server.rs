use crate::matching::{MatchResult, match_request};

use pact_matching::models::{RequestResponsePact, Request, OptionalBody, HttpPart};
use pact_matching::models::parse_query_string;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use log::{error, warn, info, debug};
use hyper::{Body, Response, Server, Error};
use hyper::http::response::{Builder as ResponseBuilder};
use hyper::http::header::{HeaderName, HeaderValue};
use hyper::service::service_fn;
use hyper::service::make_service_fn;
use serde_json::json;
use maplit::*;
use hyper::server::conn::AddrIncoming;
use rustls::ServerConfig;
use crate::mock_server::{MockServer, MockServerConfig};

#[derive(Debug, Clone)]
enum InteractionError {
    RequestHeaderEncodingError,
    RequestBodyError,
    ResponseHeaderEncodingError,
    ResponseBodyError
}

fn extract_path(uri: &hyper::Uri) -> String {
    uri.path_and_query()
        .map(|path_and_query| path_and_query.path())
        .unwrap_or("/")
        .into()
}

fn extract_query_string(uri: &hyper::Uri) -> Option<HashMap<String, Vec<String>>> {
    uri.path_and_query()
        .and_then(|path_and_query| path_and_query.query())
        .and_then(|query| parse_query_string(&query.into()))
}

fn extract_headers(headers: &hyper::HeaderMap) -> Result<Option<HashMap<String, Vec<String>>>, InteractionError> {
  if !headers.is_empty() {
    let result: Result<HashMap<String, Vec<String>>, InteractionError> = headers.keys()
      .map(|name| -> Result<(String, Vec<String>), InteractionError> {
        let values = headers.get_all(name);
        let parsed_vals: Vec<Result<String, InteractionError>> = values.iter()
          .map(|val| val.to_str()
            .map(|v| v.to_string())
            .map_err(|err| {
              warn!("Failed to parse HTTP header value: {}", err);
              InteractionError::RequestHeaderEncodingError
            })
          ).collect();
        if parsed_vals.iter().find(|val| val.is_err()).is_some() {
          Err(InteractionError::RequestHeaderEncodingError)
        } else {
          Ok((name.as_str().into(), parsed_vals.iter().cloned()
            .map(|val| val.unwrap_or_default())
            .flat_map(|val| val.split(",").map(|v| v.to_string()).collect::<Vec<String>>())
            .map(|val| val.trim().to_string())
            .collect()))
        }
      })
      .collect();

    result.map(|map| Some(map))
  } else {
    Ok(None)
  }
}

fn extract_body(bytes: bytes::Bytes, request: &Request) -> OptionalBody {
    if bytes.len() > 0 {
      OptionalBody::Present(bytes.to_vec(), request.content_type())
    } else {
      OptionalBody::Empty
    }
}

async fn hyper_request_to_pact_request(req: hyper::Request<Body>) -> Result<Request, InteractionError> {
    let method = req.method().to_string();
    let path = extract_path(req.uri());
    let query = extract_query_string(req.uri());
    let headers = extract_headers(req.headers())?;

    let body_bytes = hyper::body::to_bytes(req.into_body())
        .await
        .map_err(|_| InteractionError::RequestBodyError)?;

    let request = Request {
      method,
      path,
      query,
      headers,
      .. Request::default()
    };

    Ok(Request {
      body: extract_body(body_bytes, &request),
      .. request.clone()
    })
}

fn set_hyper_headers(builder: &mut ResponseBuilder, headers: &Option<HashMap<String, Vec<String>>>) -> Result<(), InteractionError> {
    let hyper_headers = builder.headers_mut().unwrap();
    match headers {
        Some(header_map) => {
            for (k, v) in header_map {
                for val in v {
                    // FIXME?: Headers are not sent in "raw" mode.
                    // Names are converted to lower case and values are parsed.
                    hyper_headers.append(
                        HeaderName::from_bytes(k.as_bytes())
                            .map_err(|err| {
                                error!("Invalid header name '{}' ({})", k, err);
                                InteractionError::ResponseHeaderEncodingError
                            })?,
                        val.parse::<HeaderValue>()
                            .map_err(|err| {
                                error!("Invalid header value '{}': '{}' ({})", k, val, err);
                                InteractionError::ResponseHeaderEncodingError
                            })?
                    );
                }
            }
        },
        _ => {}
    }
    Ok(())
}

fn error_body(request: &Request, error: &String) -> String {
    let body = json!({ "error" : format!("{} : {:?}", error, request) });
    body.to_string()
}

fn match_result_to_hyper_response(
  request: &Request,
  match_result: MatchResult,
  config: &MockServerConfig
) -> Result<Response<Body>, InteractionError> {
  let cors_preflight = config.cors_preflight;

  match match_result {
    MatchResult::RequestMatch(ref interaction) => {
      let response = pact_matching::generate_response(&interaction.response, &hashmap!{});
      info!("Request matched, sending response {}", response);
      if interaction.response.has_text_body() {
        debug!("     body: '{}'", interaction.response.body.str_value());
      }

      let mut builder = Response::builder()
        .status(response.status)
        .header(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(hyper::header::ACCESS_CONTROL_ALLOW_HEADERS, "*")
        .header(hyper::header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, POST, PUT, DELETE, CONNECT, OPTIONS, TRACE, PATCH")
        .header(hyper::header::ACCESS_CONTROL_EXPOSE_HEADERS, "Location, Link");

      set_hyper_headers(&mut builder, &response.headers)?;

      builder.body(match response.body {
        OptionalBody::Present(ref s, _) => Body::from(s.clone()),
        _ => Body::empty()
      })
        .map_err(|_| InteractionError::ResponseBodyError)
    },
    _ => {
      debug!("Request did not match: {}", match_result);
      if cors_preflight && request.method.to_uppercase() == "OPTIONS" {
        info!("Responding to CORS pre-flight request");
        let origin = match request.headers.clone() {
          Some(ref h) => h.iter()
            .find(|kv| kv.0.to_lowercase() == "referer")
            .map(|kv| kv.1.clone().join(", ")).unwrap_or("*".to_string()),
          None => "*".to_string()
        };
        let cors_headers = match request.headers.clone() {
          Some(ref h) => h.iter()
            .find(|kv| kv.0.to_lowercase() == "access-control-request-headers")
            .map(|kv| kv.1.clone().join(", ") + ", *").unwrap_or("*".to_string()),
          None => "*".to_string()
        };

        Response::builder()
          .status(204)
          .header(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, origin)
          .header(hyper::header::ACCESS_CONTROL_ALLOW_METHODS, "GET, HEAD, POST, PUT, DELETE, CONNECT, OPTIONS, TRACE, PATCH")
          .header(hyper::header::ACCESS_CONTROL_ALLOW_HEADERS, cors_headers)
          .header(hyper::header::ACCESS_CONTROL_EXPOSE_HEADERS, "Location, Link")
          .body(Body::empty())
          .map_err(|_| InteractionError::ResponseBodyError)
      } else {
        Response::builder()
          .status(500)
          .header(hyper::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
          .header(hyper::header::CONTENT_TYPE, "application/json; charset=utf-8")
          .header("X-Pact", match_result.match_key())
          .body(Body::from(error_body(&request, &match_result.match_key())))
          .map_err(|_| InteractionError::ResponseBodyError)
      }
    }
  }
}

async fn handle_request(
  req: hyper::Request<Body>,
  pact: Arc<RequestResponsePact>,
  matches: Arc<Mutex<Vec<MatchResult>>>,
  config: Arc<MockServerConfig>
) -> Result<Response<Body>, InteractionError> {
    debug!("Creating pact request from hyper request");

    let pact_request = hyper_request_to_pact_request(req).await?;
    info!("Received request {}", pact_request);
    if pact_request.has_text_body() {
      debug!("     body: '{}'", pact_request.body.str_value());
    }

    let match_result = match_request(&pact_request, &pact.interactions);

    matches.lock().unwrap().push(match_result.clone());

    match_result_to_hyper_response(&pact_request, match_result, &config)
}

// TODO: Should instead use some form of X-Pact headers
fn handle_mock_request_error(result: Result<Response<Body>, InteractionError>) -> Result<Response<Body>, Error> {
    match result {
        Ok(response) => Ok(response),
        Err(error) => {
            let response = match error {
                InteractionError::RequestHeaderEncodingError => Response::builder()
                    .status(400)
                    .body(Body::from("Found an invalid header encoding")),
                InteractionError::RequestBodyError => Response::builder()
                    .status(500)
                    .body(Body::from("Could not process request body")),
                InteractionError::ResponseBodyError => Response::builder()
                    .status(500)
                    .body(Body::from("Could not process response body")),
                InteractionError::ResponseHeaderEncodingError => Response::builder()
                    .status(500)
                    .body(Body::from("Could not set response header"))
            };
            Ok(response.unwrap())
        }
    }
}

// Create and bind the server, but do not start it.
// Returns a future that drives the server.
// The reason that the function itself is still async (even if it performs
// no async operations) is that it needs a tokio context to be able to call try_bind.
pub(crate) async fn create_and_bind(
  pact: RequestResponsePact,
  addr: std::net::SocketAddr,
  shutdown: impl std::future::Future<Output = ()>,
  matches: Arc<Mutex<Vec<MatchResult>>>,
  mock_server: &MockServer
) -> Result<(impl std::future::Future<Output = ()>, std::net::SocketAddr), hyper::Error> {
  let pact = Arc::new(pact);
  let config = Arc::new(mock_server.config.clone());

  let server = Server::try_bind(&addr)?
    .serve(make_service_fn(move |_| {
      let pact = pact.clone();
      let matches = matches.clone();
      let config = config.clone();

      async {
        Ok::<_, hyper::Error>(
          service_fn(move |req| {
            let pact = pact.clone();
            let matches = matches.clone();
            let config = config.clone();

            async {
              handle_mock_request_error(
                handle_request(req, pact, matches, config).await
              )
            }
          })
        )
      }
    }));

  let socket_addr = server.local_addr();

  Ok((
      // This is the future that drives the server:
      async {
          let _ = server
              .with_graceful_shutdown(shutdown)
              .await;
      },
      socket_addr
  ))
}

pub(crate) async fn create_and_bind_tls(
  pact: RequestResponsePact,
  addr: std::net::SocketAddr,
  shutdown: impl std::future::Future<Output = ()>,
  matches: Arc<Mutex<Vec<MatchResult>>>,
  tls: &ServerConfig,
  mock_server: &MockServer
) -> Result<(impl std::future::Future<Output = ()>, std::net::SocketAddr), hyper::Error> {
  let pact = Arc::new(pact);
  let config = Arc::new(mock_server.config.clone());

  let incoming = AddrIncoming::bind(&addr)?;
  let socket_addr = incoming.local_addr();
  let server = Server::builder(crate::tls::TlsAcceptor::new(tls.clone(), incoming))
    .serve(make_service_fn(move |_| {
      let pact = pact.clone();
      let matches = matches.clone();
      let config = config.clone();

      async {
        Ok::<_, hyper::Error>(
          service_fn(move |req| {
            let pact = pact.clone();
            let matches = matches.clone();
            let config = config.clone();

            async {
              handle_mock_request_error(
                handle_request(req, pact, matches, config).await
              )
            }
          })
        )
      }
    }));

  Ok((
    // This is the future that drives the server:
    async {
      let _ = server
        .with_graceful_shutdown(shutdown)
        .await;
    },
    socket_addr
  ))
}

#[cfg(test)]
mod tests {
  use super::*;
  use hyper::HeaderMap;
  use hyper::header::{ACCEPT, USER_AGENT, CONTENT_TYPE};
  use expectest::prelude::*;
  use expectest::expect;

  #[tokio::test]
  async fn can_fetch_results_on_current_thread() {
    let (shutdown_tx, shutdown_rx) = futures::channel::oneshot::channel();
    let matches = Arc::new(Mutex::new(vec![]));

    let (future, _) = create_and_bind(
      RequestResponsePact::default(),
      ([0, 0, 0, 0], 0 as u16).into(),
      async {
          shutdown_rx.await.ok();
      },
      matches.clone(),
      &MockServer::default()
    ).await.unwrap();

    let join_handle = tokio::task::spawn(future);

    shutdown_tx.send(()).unwrap();

    // Server has shut down, now flush the server future from runtime
    join_handle.await.unwrap();

    // 0 matches have been produced
    let all_matches = matches.lock().unwrap().clone();
    assert_eq!(all_matches, vec![]);
  }

  #[test]
  fn handle_hyper_headers_with_multiple_values() {
    let mut headers = HeaderMap::new();
    headers.append(ACCEPT, "application/xml, application/json".parse().unwrap());
    headers.append(USER_AGENT, "test".parse().unwrap());
    headers.append(USER_AGENT, "test2".parse().unwrap());
    headers.append(CONTENT_TYPE, "text/plain".parse().unwrap());
    let result = extract_headers(&headers);
    expect!(result).to(be_ok().value(Some(hashmap! {
      "accept".to_string() => vec!["application/xml".to_string(), "application/json".to_string()],
      "user-agent".to_string() => vec!["test".to_string(), "test2".to_string()],
      "content-type".to_string() => vec!["text/plain".to_string()]
    })));
  }
}
