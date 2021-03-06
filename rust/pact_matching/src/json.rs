//! The `json` module provides functions to compare and display the differences between JSON bodies

use std::str::FromStr;

use ansi_term::Colour::*;
use difference::*;
use log::*;
use onig::Regex;
use serde_json::{json, Value};

use crate::{MatchingContext, merge_result};
use crate::binary_utils::{convert_data, match_content_type};
use crate::matchers::*;
use crate::models::HttpPart;
use crate::models::json_utils::json_to_string;
use crate::models::matchingrules::*;
use crate::time_utils::validate_datetime;

use super::Mismatch;

fn type_of(json: &Value) -> String {
  match json {
    &Value::Object(_) => "Map",
    &Value::Array(_) => "List",
    &Value::Null => "Null",
    &Value::Bool(_) => "Boolean",
    &Value::Number(_) => "Number",
    &Value::String(_) => "String"
  }.to_string()
}

impl Matches<Value> for Value {
  fn matches(&self, actual: &Value, matcher: &MatchingRule) -> Result<(), String> {
    let result = match *matcher {
      MatchingRule::Regex(ref regex) => {
        match Regex::new(regex) {
          Ok(re) => {
            let actual_str = match actual {
              &Value::String(ref s) => s.clone(),
              _ => actual.to_string()
            };
            if re.is_match(&actual_str) {
              Ok(())
            } else {
              Err(format!("Expected '{}' to match '{}'", json_to_string(actual), regex))
            }
          },
          Err(err) => Err(format!("'{}' is not a valid regular expression - {}", regex, err))
        }
      },
      MatchingRule::Include(ref substr) => {
        let actual_str = match actual {
          &Value::String(ref s) => s.clone(),
          _ => actual.to_string()
        };
        if actual_str.contains(substr) {
          Ok(())
        } else {
          Err(format!("Expected '{}' to include '{}'", json_to_string(actual), substr))
        }
      },
      MatchingRule::Type => {
        match (self, actual) {
          (&Value::Array(_), &Value::Array(_)) => Ok(()),
          (&Value::Bool(_), &Value::Bool(_)) => Ok(()),
          (&Value::Number(_), &Value::Number(_)) => Ok(()),
          (&Value::Null, &Value::Null) => Ok(()),
          (&Value::Object(_), &Value::Object(_)) => Ok(()),
          (&Value::String(_), &Value::String(_)) => Ok(()),
          (_, _) => Err(format!("Expected '{}' to be the same type as '{}'", json_to_string(self), json_to_string(actual))),
        }
      },
      MatchingRule::MinType(min) => {
        match (self, actual) {
          (&Value::Array(_), &Value::Array(ref actual_array)) => if actual_array.len() < min {
            Err(format!("Expected '{}' to have at least {} item(s)", json_to_string(actual), min))
          } else {
            Ok(())
          },
          (&Value::Bool(_), &Value::Bool(_)) => Ok(()),
          (&Value::Number(_), &Value::Number(_)) => Ok(()),
          (&Value::Null, &Value::Null) => Ok(()),
          (&Value::Object(_), &Value::Object(_)) => Ok(()),
          (&Value::String(_), &Value::String(_)) => Ok(()),
          (_, _) => Err(format!("Expected '{}' to be the same type as '{}'", json_to_string(self), json_to_string(actual))),
        }
      },
      MatchingRule::MaxType(max) => {
        match (self, actual) {
          (&Value::Array(_), &Value::Array(ref actual_array)) => if actual_array.len() > max {
            Err(format!("Expected '{}' to have at most {} item(s)", json_to_string(actual), max))
          } else {
            Ok(())
          },
          (&Value::Bool(_), &Value::Bool(_)) => Ok(()),
          (&Value::Number(_), &Value::Number(_)) => Ok(()),
          (&Value::Null, &Value::Null) => Ok(()),
          (&Value::Object(_), &Value::Object(_)) => Ok(()),
          (&Value::String(_), &Value::String(_)) => Ok(()),
          (_, _) => Err(format!("Expected '{}' to be the same type as '{}'", json_to_string(self), json_to_string(actual))),
        }
      },
      MatchingRule::MinMaxType(min, max) => {
        match (self, actual) {
          (&Value::Array(_), &Value::Array(ref actual_array)) => if actual_array.len() < min {
            Err(format!("Expected '{}' to have at least {} item(s)", json_to_string(actual), min))
          } else if actual_array.len() > max {
            Err(format!("Expected '{}' to have at most {} item(s)", json_to_string(actual), max))
          } else {
            Ok(())
          },
          (&Value::Bool(_), &Value::Bool(_)) => Ok(()),
          (&Value::Number(_), &Value::Number(_)) => Ok(()),
          (&Value::Null, &Value::Null) => Ok(()),
          (&Value::Object(_), &Value::Object(_)) => Ok(()),
          (&Value::String(_), &Value::String(_)) => Ok(()),
          (_, _) => Err(format!("Expected '{}' to be the same type as '{}'", json_to_string(self), json_to_string(actual))),
        }
      },
      MatchingRule::Equality => {
        if self == actual {
          Ok(())
        } else {
          Err(format!("Expected '{}' to be equal to '{}'", json_to_string(self), json_to_string(actual)))
        }
      },
      MatchingRule::Null => match actual {
        &Value::Null => Ok(()),
        _ => Err(format!("Expected '{}' to be a null value", json_to_string(actual)))
      },
      MatchingRule::Integer => if actual.is_i64() || actual.is_u64() {
        Ok(())
      } else {
        Err(format!("Expected '{}' to be an integer value", json_to_string(actual)))
      },
      MatchingRule::Decimal => if actual.is_f64() {
        Ok(())
      } else {
        Err(format!("Expected '{}' to be a decimal value", json_to_string(actual)))
      },
      MatchingRule::Number => if actual.is_number() {
        Ok(())
      } else {
        Err(format!("Expected '{}' to be a number", json_to_string(actual)))
      },
      MatchingRule::Date(ref s) => {
        validate_datetime(&json_to_string(actual), s)
          .map_err(|err| format!("Expected '{}' to match a date format of '{}': {}", actual, s, err))
      },
      MatchingRule::Time(ref s) => {
        validate_datetime(&json_to_string(actual), s)
          .map_err(|err| format!("Expected '{}' to match a time format of '{}': {}", actual, s, err))
      },
      MatchingRule::Timestamp(ref s) => {
        validate_datetime(&json_to_string(actual), s)
          .map_err(|err| format!("Expected '{}' to match a timestamp format of '{}': {}", actual, s, err))
      },
      MatchingRule::ContentType(ref expected_content_type) => {
        match_content_type(&convert_data(actual), expected_content_type)
          .map_err(|err| format!("Expected data to have a content type of '{}' but was {}", expected_content_type, err))
      }
      _ => Ok(())
    };
    debug!("JSON -> JSON: Comparing '{}' to '{}' using {:?} -> {:?}", self, actual, matcher, result);
    result
  }
}

/// Matches the expected JSON to the actual, and populates the mismatches vector with any differences
pub fn match_json(expected: &dyn HttpPart, actual: &dyn HttpPart, context: &MatchingContext) -> Result<(), Vec<super::Mismatch>> {
  let expected_json = serde_json::from_slice(expected.body().value().as_slice());
  let actual_json = serde_json::from_slice(actual.body().value().as_slice());

  if expected_json.is_err() || actual_json.is_err() {
    let mut mismatches = vec![];
    match expected_json {
      Err(e) => {
        mismatches.push(Mismatch::BodyMismatch {
          path: "$".to_string(),
          expected: Some(expected.body().value().clone().into()),
          actual: Some(actual.body().value().clone().into()),
          mismatch: format!("Failed to parse the expected body: '{}'", e),
        });
      },
      _ => ()
    }
    match actual_json {
      Err(e) => {
        mismatches.push(Mismatch::BodyMismatch {
          path: "$".to_string(),
          expected: Some(expected.body().value().clone().into()),
          actual: Some(actual.body().value().clone().into()),
          mismatch: format!("Failed to parse the actual body: '{}'", e),
        });
      },
      _ => ()
    }
    Err(mismatches.clone())
  } else {
    compare(&vec!["$"], &expected_json.unwrap(), &actual_json.unwrap(), context)
  }
}

fn walk_json(json: &Value, path: &mut dyn Iterator<Item=&str>) -> Option<Value> {
  match path.next() {
    Some(p) => match json {
      &Value::Object(_) => json.get(p).map(|json| json.clone()),
      &Value::Array(ref array) => match usize::from_str(p) {
        Ok(index) => array.get(index).map(|json| json.clone()),
        Err(_) => None
      },
      _ => None
    },
    None => None
  }
}

/// Returns a diff of the expected versus the actual JSON bodies, focusing on a particular path
pub fn display_diff(expected: &String, actual: &String, path: &str, indent: &str) -> String {
  let expected_body = Value::from_str(expected).unwrap();
  let actual_body = Value::from_str(actual).unwrap();
  let mut path = path.split('.').skip(1);
  let next = path.next();
  let expected_fragment = if next.is_none() {
    serde_json::to_string_pretty(&expected_body).unwrap_or_default()
  } else {
    match walk_json(&expected_body, &mut path.clone()) {
      Some(json) => format!("{:?}", serde_json::to_string_pretty(&json)),
      None => s!("")
    }
  };
  let actual_fragment = if next.is_none() {
    serde_json::to_string_pretty(&actual_body).unwrap_or_default()
  } else {
    match walk_json(&actual_body, &mut path.clone()) {
      Some(json) => format!("{:?}", serde_json::to_string_pretty(&json)),
      None => s!("")
    }
  };
  let changeset = Changeset::new(&expected_fragment, &actual_fragment, "\n");
  let mut output = String::new();
  for change in changeset.diffs {
      match change {
          Difference::Same(ref x) => output.push_str(&format!("{}{}\n", indent, x)),
          Difference::Add(ref x) => output.push_str(&Green.paint(format!("{}+{}\n", indent, x)).to_string()),
          Difference::Rem(ref x) => output.push_str(&Red.paint(format!("{}-{}\n", indent, x)).to_string())
      }
  }
  output
}

fn compare(path: &Vec<&str>, expected: &Value, actual: &Value, context: &MatchingContext) -> Result<(), Vec<Mismatch>> {
  log::debug!("Comparing path {}", path.join("."));
  match (expected, actual) {
    (&Value::Object(ref emap), &Value::Object(ref amap)) => compare_maps(path, emap, amap, context),
    (&Value::Object(_), _) => {
      Err(vec![ Mismatch::BodyMismatch {
        path: path.join("."),
        expected: Some(json_to_string(expected).into()),
        actual: Some(json_to_string(actual).into()),
        mismatch: format!("Type mismatch: Expected {} {} but received {} {}",
                          type_of(expected), expected, type_of(actual), actual),
      } ])
    }
    (&Value::Array(ref elist), &Value::Array(ref alist)) => compare_lists(path, elist, alist, context),
    (&Value::Array(_), _) => {
      Err(vec![ Mismatch::BodyMismatch {
        path: path.join("."),
        expected: Some(json_to_string(expected).into()),
        actual: Some(json_to_string(actual).into()),
        mismatch: format!("Type mismatch: Expected {} {} but received {} {}",
                          type_of(expected), json_to_string(expected), type_of(actual), json_to_string(actual)),
      } ])
    }
    (_, _) => compare_values(path, expected, actual, context)
  }
}

fn compare_maps(path: &Vec<&str>, expected: &serde_json::Map<String, Value>, actual: &serde_json::Map<String, Value>,
                context: &MatchingContext) -> Result<(), Vec<Mismatch>> {
  if expected.is_empty() && !actual.is_empty() {
    Err(vec![ Mismatch::BodyMismatch {
      path: path.join("."),
      expected: Some(json_to_string(&json!(expected)).into()),
      actual: Some(json_to_string(&json!(actual)).into()),
      mismatch: format!("Expected an empty Map but received {}", json_to_string(&json!(actual))),
    } ])
  } else {
    let mut result = Ok(());
    let expected = expected.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let actual = actual.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

    if context.matcher_is_defined(path) {
      for matcher in context.select_best_matcher(path).unwrap().rules {
        result = merge_result(result,matcher.compare_maps(path, &expected, &actual, &context, &mut |p, expected, actual| {
          compare(&p, expected, actual, context)
        }));
      }
    } else {
      result = merge_result(result, context.match_keys(path, &expected, &actual));
      for (key, value) in expected.iter() {
        let mut p = path.to_vec();
        p.push(key.as_str());
        if actual.contains_key(key) {
          result = merge_result(result, compare(&p, value, &actual[key], context));
        }
      }
    };
    result
  }
}

fn compare_lists(path: &Vec<&str>, expected: &Vec<Value>, actual: &Vec<Value>,
                 context: &MatchingContext) -> Result<(), Vec<Mismatch>> {
  let spath = path.join(".");
  if context.matcher_is_defined(&path) {
    log::debug!("compare_lists: matcher defined for path '{}'", spath);
    let mut result = Ok(());
    for matcher in context.select_best_matcher(path).unwrap().rules {
      let values_result = matcher.compare_lists(path, expected, actual, context, &|p, expected, actual, context| {
        compare(&p, expected, actual, context)
      });
      result = merge_result(result, values_result);
    }
    result
  } else {
    if expected.is_empty() && !actual.is_empty() {
      Err(vec![ Mismatch::BodyMismatch {
        path: spath,
        expected: Some(json_to_string(&json!(expected)).into()),
        actual: Some(json_to_string(&json!(actual)).into()),
        mismatch: format!("Expected an empty List but received {}", json_to_string(&json!(actual))),
      } ])
    } else {
      let result = compare_list_content(path, expected, actual, context);
      if expected.len() != actual.len() {
        merge_result(result, Err(vec![ Mismatch::BodyMismatch {
          path: spath,
          expected: Some(json_to_string(&json!(expected)).into()),
          actual: Some(json_to_string(&json!(actual)).into()),
          mismatch: format!("Expected a List with {} elements but received {} elements",
                            expected.len(), actual.len()),
        } ]))
      } else {
        result
      }
    }
  }
}

fn compare_list_content(path: &Vec<&str>, expected: &Vec<Value>, actual: &Vec<Value>, context: &MatchingContext) -> Result<(), Vec<Mismatch>> {
  let mut result = Ok(());
  for (index, value) in expected.iter().enumerate() {
    let ps = index.to_string();
    log::debug!("Comparing list item {} with value '{:?}' to '{:?}'", index, actual.get(index), value);
    let mut p = path.to_vec();
    p.push(ps.as_str());
    if index < actual.len() {
      result = merge_result(result, compare(&p, value, &actual[index], context));
    } else if !context.matcher_is_defined(&p) {
      result = merge_result(result,Err(vec![ Mismatch::BodyMismatch { path: path.join("."),
        expected: Some(json_to_string(&json!(expected)).into()),
        actual: Some(json_to_string(&json!(actual)).into()),
        mismatch: format!("Expected {} but was missing", json_to_string(value)) } ]))
    }
  }
  result
}

fn compare_values(path: &Vec<&str>, expected: &Value, actual: &Value, context: &MatchingContext) -> Result<(), Vec<Mismatch>> {
  let matcher_result = if context.matcher_is_defined(&path) {
    debug!("Calling match_values for path {}", path.join("."));
    match_values(path, context, expected, actual)
  } else {
    expected.matches(actual, &MatchingRule::Equality).map_err(|err| vec![err])
  };
  log::debug!("Comparing '{:?}' to '{:?}' at path '{}' -> {:?}", expected, actual, path.join("."), matcher_result);
  matcher_result.map_err(|messages| {
    messages.iter().map(|message| {
      Mismatch::BodyMismatch {
        path: path.join("."),
        expected: Some(format!("{}", expected).into()),
        actual: Some(format!("{}", actual).into()),
        mismatch: message.clone()
      }
    }).collect()
  })
}

#[cfg(test)]
mod tests {
  use expectest::expect;
  use expectest::prelude::*;

  use crate::DiffConfig;
  use crate::Mismatch;
  use crate::models::{OptionalBody, Request};

  use super::*;
  use crate::Mismatch::BodyMismatch;

  macro_rules! request {
    ($e:expr) => (Request { body: OptionalBody::Present($e.as_bytes().to_vec(), None), .. Request::default() })
  }

  #[test]
  fn match_json_handles_invalid_expected_json() {
    let expected = request!(r#"{"json": "is bad"#);
    let actual = request!("{}");
    let result = match_json(&expected.clone(), &actual.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_err().value(vec![Mismatch::BodyMismatch { path: s!("$"), expected: Some(expected.body.value()),
      actual: Some(actual.body.value()), mismatch: s!("") }]));
  }

  #[test]
  fn match_json_handles_invalid_actual_json() {
    let expected = request!("{}");
    let actual = request!(r#"{json: "is bad"}"#);
    let result = match_json(&expected.clone(), &actual.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_err().value(
      vec![
        Mismatch::BodyMismatch {
          path: s!("$"),
          expected: Some(expected.body.value()),
          actual: Some(actual.body.value()),
          mismatch: s!("Type mismatch: Expected List [{}] but received Map {}")
        }
      ]
    ));
  }

  fn mismatch_message(mismatch: &Result<(), Vec<Mismatch>>) -> String {
    match mismatch {
      Err(mismatches) => match &mismatches.first() {
        Some(Mismatch::BodyMismatch { mismatch, .. }) => mismatch.clone(),
        _ => "".into()
      },
      _ => "".into()
    }
  }

  #[test]
  fn match_json_handles_expecting_a_map_but_getting_a_list() {
    let expected = request!(r#"{}"#);
    let actual = request!(r#"[]"#);
    let result = match_json(&expected.clone(), &actual.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Type mismatch: Expected Map {} but received List []")));
    expect!(result).to(be_err().value(vec![Mismatch::BodyMismatch { path: s!("$"), expected: Some(expected.body.value()),
      actual: Some(actual.body.value()), mismatch: s!("") }]));
  }

  #[test]
  fn match_json_handles_expecting_a_list_but_getting_a_map() {
    let expected = request!(r#"[{}]"#);
    let actual = request!(r#"{}"#);
    let result = match_json(&expected.clone(), &actual.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Type mismatch: Expected List [{}] but received Map {}")));
    expect!(result).to(be_err().value(vec![ Mismatch::BodyMismatch { path: s!("$"), expected: Some(expected.body.value()),
      actual: Some(actual.body.value()), mismatch: s!("") }]));
  }

  #[test]
  fn match_json_handles_comparing_strings() {
    let val1 = request!(r#""string value""#);
    let val2 = request!(r#""other value""#);
    let result = match_json(&val1.clone(), &val1.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_ok());

    let result = match_json(&val1.clone(), &val2.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Expected 'string value' to be equal to 'other value'")));
    expect!(result).to(be_err().value(vec![ Mismatch::BodyMismatch { path: s!("$"), expected: Some(val1.body.value()),
      actual: Some(val2.body.value()), mismatch: s!("")} ]));
  }

  #[test]
  fn match_json_handles_comparing_integers() {
    let val1 = request!(r#"100"#);
    let val2 = request!(r#"200"#);
    let result = match_json(&val1.clone(), &val1.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_ok());

    let result = match_json(&val1.clone(), &val2.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Expected '100' to be equal to '200'")));
    expect!(result).to(be_err().value(vec![ Mismatch::BodyMismatch { path: s!("$"), expected: Some(val1.body.value()),
      actual: Some(val2.body.value()), mismatch: s!("") } ]));
  }

  #[test]
  fn match_json_handles_comparing_floats() {
    let val1 = request!(r#"100.01"#);
    let val2 = request!(r#"100.02"#);
    let result = match_json(&val1.clone(), &val1.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_ok());

    let result = match_json(&val1.clone(), &val2.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Expected '100.01' to be equal to '100.02'")));
    expect!(result).to(be_err().value(vec![ Mismatch::BodyMismatch { path: s!("$"), expected: Some(val1.body.value()),
      actual: Some(val2.body.value()), mismatch: s!("") } ]));
  }

  #[test]
  fn match_json_handles_comparing_booleans() {
    let val1 = request!(r#"true"#);
    let val2 = request!(r#"false"#);
    let result = match_json(&val1.clone(), &val1.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_ok());

    let result = match_json(&val1.clone(), &val2.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Expected 'true' to be equal to 'false'")));
    expect!(result).to(be_err().value(vec![ Mismatch::BodyMismatch { path: s!("$"), expected: Some(val1.body.value()),
        actual: Some(val2.body.value()), mismatch: s!("") } ]));
  }

  #[test]
  fn match_json_handles_comparing_nulls() {
    let val1 = request!(r#"null"#);
    let val2 = request!(r#"33"#);
    let result = match_json(&val1.clone(), &val1.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_ok());

    let result = match_json(&val1.clone(), &val2.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Expected 'null' to be equal to '33'")));
    expect!(result).to(be_err().value(vec![ Mismatch::BodyMismatch { path: s!("$"), expected: Some(val1.clone().body.value()),
        actual: Some(val2.clone().body.value()), mismatch: s!("") } ]));
  }

  #[test]
  fn match_json_handles_comparing_lists() {
    let val1 = request!(r#"[]"#);
    let val2 = request!(r#"[11,22,33]"#);
    let val3 = request!(r#"[11,44,33]"#);
    let val4 = request!(r#"[11,44,33, 66]"#);

    let result = match_json(&val1.clone(), &val1.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_ok());

    let result = match_json(&val2.clone(), &val2.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_ok());

    let result = match_json(&val3.clone(), &val3.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_ok());

    let result = match_json(&val1.clone(), &val2.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Expected an empty List but received [11,22,33]")));
    expect!(result).to(be_err());

    let result = match_json(&val2.clone(), &val3.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Expected '22' to be equal to '44'")));
    expect!(result).to(be_err().value(vec![ Mismatch::BodyMismatch { path: s!("$.1"),
        expected: Some("22".into()), actual: Some("44".into()), mismatch: s!("") } ]));

    let result = match_json(&val3.clone(), &val4.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Expected a List with 3 elements but received 4 elements")));
    expect!(result).to(be_err().value(vec![ Mismatch::BodyMismatch { path: s!("$"),
        expected: Some("[11,44,33]".into()),
        actual: Some("[11,44,33,66]".into()), mismatch: s!("") } ]));

    let result = match_json(&val2.clone(), &val4.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    let mismatches = result.unwrap_err();
    expect!(mismatches.iter()).to(have_count(2));
    let mismatch = mismatches[0].clone();
    expect!(&mismatch).to(be_equal_to(&Mismatch::BodyMismatch { path: s!("$.1"),
        expected: Some("22".into()),
        actual: Some("44".into()), mismatch: s!("")}));
    expect!(mismatch.description()).to(be_equal_to(s!("$.1 -> Expected '22' to be equal to '44'")));
    let mismatch = mismatches[1].clone();
    expect!(&mismatch).to(be_equal_to(&Mismatch::BodyMismatch { path: s!("$"),
        expected: Some("[11,22,33]".into()),
        actual: Some("[11,44,33,66]".into()), mismatch: s!("")}));
    expect!(mismatch.description()).to(be_equal_to(s!("$ -> Expected a List with 3 elements but received 4 elements")));

    let result = match_json(&val2.clone(), &val4.clone(), &MatchingContext::new(DiffConfig::AllowUnexpectedKeys, &matchingrules!{
        "body" => {
            "$" => [ MatchingRule::Type ]
        }
    }.rules_for_category("body").unwrap()));
    expect!(result).to(be_ok());
    let result = match_json(&val4, &val2, &MatchingContext::new(DiffConfig::AllowUnexpectedKeys, &matchingrules!{
        "body" => {
            "$" => [ MatchingRule::Type ]
        }
    }.rules_for_category("body").unwrap()));
    expect!(result).to(be_ok());
  }

  #[test]
  fn match_json_handles_comparing_maps() {
    let val1 = request!(r#"{}"#);
    let val2 = request!(r#"{"a": 1, "b": 2}"#);
    let val3 = request!(r#"{"a": 1, "b": 3}"#);
    let val4 = request!(r#"{"a": 1, "b": 2, "c": 3}"#);

    let result = match_json(&val1.clone(), &val1.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_ok());

    let result = match_json(&val2.clone(), &val2.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_ok());

    let result = match_json(&val4.clone(), &val4.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_ok());

    let result = match_json(&val1.clone(), &val2.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Expected an empty Map but received {\"a\":1,\"b\":2}")));

    let result = match_json(&val2.clone(), &val3.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Expected '2' to be equal to '3'")));
    expect!(result).to(be_err().value(vec![ Mismatch::BodyMismatch { path: s!("$.b"),
        expected: Some("2".into()), actual: Some("3".into()), mismatch: s!("") } ]));

    let result = match_json(&val2.clone(), &val4.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_ok());

    let result = match_json(&val2.clone(), &val4.clone(), &MatchingContext::with_config(DiffConfig::NoUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Expected a Map with keys a, b but received one with keys a, b, c")));
    expect!(result).to(be_err().value(vec![ Mismatch::BodyMismatch { path: s!("$"),
        expected: Some("{\"a\":\"1\",\"b\":\"2\"}".into()),
        actual: Some("{\"a\":\"1\",\"b\":\"2\",\"c\":\"3\"}".into()), mismatch: "Expected a Map with keys a, b but received one with keys a, b, c".to_string()
    } ]));

    let result = match_json(&val3.clone(), &val4.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(mismatch_message(&result)).to(be_equal_to(s!("Expected '3' to be equal to '2'")));
    expect!(result).to(be_err().value(vec![ Mismatch::BodyMismatch { path: s!("$.b"),
        expected: Some("3".into()),
        actual: Some("2".into()), mismatch: s!("") } ]));

    let result = match_json(&val3.clone(), &val4.clone(), &MatchingContext::with_config(DiffConfig::NoUnexpectedKeys));
    let mismatches = result.unwrap_err();
    expect!(mismatches.iter()).to(have_count(2));
    let mismatch = mismatches[0].clone();
    expect!(&mismatch).to(be_equal_to(&Mismatch::BodyMismatch { path: s!("$"),
        expected: Some("{\"a\":\"1\",\"b\":\"3\"}".into()),
        actual: Some("{\"a\":\"1\",\"b\":\"2\",\"c\":\"3\"}".into()), mismatch: s!("")}));
    expect!(mismatch.description()).to(be_equal_to(s!("$ -> Expected a Map with keys a, b but received one with keys a, b, c")));
    let mismatch = mismatches[1].clone();
    expect!(&mismatch).to(be_equal_to(&Mismatch::BodyMismatch { path: s!("$.b"),
        expected: Some("3".into()),
        actual: Some("2".into()), mismatch: s!("")}));
    expect!(mismatch.description()).to(be_equal_to(s!("$.b -> Expected '3' to be equal to '2'")));

    let result = match_json(&val4.clone(), &val2.clone(), &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    let mismatches = result.unwrap_err();
    expect!(mismatches.iter()).to(have_count(1));
    let mismatch = mismatches[0].clone();
    expect!(&mismatch).to(be_equal_to(&Mismatch::BodyMismatch { path: s!("$"),
        expected: Some("{\"a\":\"1\",\"b\":\"2\",\"c\":\"3\"}".into()),
        actual: Some("{\"a\":\"1\",\"b\":\"2\"}".into()), mismatch: s!("")}));
    expect!(mismatch.description()).to(be_equal_to(s!("$ -> Actual map is missing the following keys: c")));

    let result = match_json(&val3, &val2, &MatchingContext::new(DiffConfig::AllowUnexpectedKeys, &matchingrules!{
      "body" => {
        "$.*" => [ MatchingRule::Type ]
      }
    }.rules_for_category("body").unwrap()));
    expect!(result).to(be_ok());
  }

    #[test]
    fn equality_matcher_test() {
        let matcher = MatchingRule::Equality;
        expect!(Value::String(s!("100")).matches(&Value::String(s!("100")), &matcher)).to(be_ok());
        expect!(Value::String(s!("100")).matches(&Value::String(s!("101")), &matcher)).to(be_err());
        expect!(Value::String(s!("100")).matches(&json!(100), &matcher)).to(be_err());
    }

    #[test]
    fn regex_matcher_test() {
        let matcher = MatchingRule::Regex(s!("^\\d+$"));
        expect!(Value::String(s!("100")).matches(&Value::String(s!("100")), &matcher)).to(be_ok());
        expect!(Value::String(s!("100")).matches(&Value::String(s!("101")), &matcher)).to(be_ok());
        expect!(Value::String(s!("100")).matches(&Value::String(s!("10a")), &matcher)).to(be_err());
        expect!(Value::String(s!("100")).matches(&json!(100), &matcher)).to(be_ok());
    }

  #[test]
  fn includes_matcher_test() {
    let matcher = MatchingRule::Include(s!("10"));
    expect!(Value::String(s!("100")).matches(&Value::String(s!("100")), &matcher)).to(be_ok());
    expect!(Value::String(s!("100")).matches(&Value::String(s!("101")), &matcher)).to(be_ok());
    expect!(Value::String(s!("100")).matches(&Value::String(s!("1a0")), &matcher)).to(be_err());
    expect!(Value::String(s!("100")).matches(&json!(100), &matcher)).to(be_ok());
  }

    #[test]
    fn type_matcher_test() {
        let matcher = MatchingRule::Type;
        expect!(Value::String(s!("100")).matches(&Value::String(s!("100")), &matcher)).to(be_ok());
        expect!(Value::String(s!("100")).matches(&Value::String(s!("101")), &matcher)).to(be_ok());
        expect!(Value::String(s!("100")).matches(&Value::String(s!("10a")), &matcher)).to(be_ok());
        expect!(Value::String(s!("100")).matches(&json!(100), &matcher)).to(be_err());
    }

    #[test]
    fn min_type_matcher_test() {
        let matcher = MatchingRule::MinType(2);
        expect!(Value::Array(vec![]).matches(&Value::Array(vec![json!(100), json!(100)]), &matcher)).to(be_ok());
        expect!(Value::Array(vec![]).matches(&Value::Array(vec![json!(100)]), &matcher)).to(be_err());
        expect!(Value::String(s!("100")).matches(&Value::String(s!("101")), &matcher)).to(be_ok());
    }

    #[test]
    fn max_type_matcher_test() {
        let matcher = MatchingRule::MaxType(1);
        expect!(Value::Array(vec![]).matches(&Value::Array(vec![json!(100), json!(100)]), &matcher)).to(be_err());
        expect!(Value::Array(vec![]).matches(&Value::Array(vec![json!(100)]), &matcher)).to(be_ok());
        expect!(Value::String(s!("100")).matches(&Value::String(s!("101")), &matcher)).to(be_ok());
    }

    #[test]
    fn min_max_type_matcher_test() {
      let matcher = MatchingRule::MinMaxType(2, 3);
      expect!(Value::Array(vec![]).matches(&Value::Array(vec![json!(100), json!(100)]),
        &matcher)).to(be_ok());
      expect!(Value::Array(vec![]).matches(&Value::Array(vec![json!(100), json!(100),
        json!(100)]), &matcher)).to(be_ok());
      expect!(Value::Array(vec![]).matches(&Value::Array(vec![json!(100), json!(100),
        json!(100), json!(100)]), &matcher)).to(be_err());
      expect!(Value::Array(vec![]).matches(&Value::Array(vec![json!(100)]), &matcher)).to(be_err());
      expect!(Value::String(s!("100")).matches(&Value::String(s!("101")), &matcher)).to(be_ok());
    }

  #[test]
  fn integer_matcher_test() {
    let matcher = MatchingRule::Integer;
    expect!(Value::String(s!("100")).matches(&Value::String(s!("100")), &matcher)).to(be_err());
    expect!(Value::String(s!("100")).matches(&json!(100), &matcher)).to(be_ok());
    expect!(Value::String(s!("100")).matches(&json!(100.02), &matcher)).to(be_err());
  }

  #[test]
  fn decimal_matcher_test() {
    let matcher = MatchingRule::Decimal;
    expect!(Value::String(s!("100")).matches(&Value::String(s!("100")), &matcher)).to(be_err());
    expect!(Value::String(s!("100")).matches(&json!(100), &matcher)).to(be_err());
    expect!(Value::String(s!("100")).matches(&json!(100.01), &matcher)).to(be_ok());
  }

  #[test]
  fn number_matcher_test() {
    let matcher = MatchingRule::Number;
    expect!(Value::String(s!("100")).matches(&Value::String(s!("100")), &matcher)).to(be_err());
    expect!(Value::String(s!("100")).matches(&json!(100), &matcher)).to(be_ok());
    expect!(Value::String(s!("100")).matches(&json!(100.01), &matcher)).to(be_ok());
  }

  #[test]
  fn null_matcher_test() {
    let matcher = MatchingRule::Null;
    expect!(Value::String(s!("100")).matches(&Value::String(s!("100")), &matcher)).to(be_err());
    expect!(Value::String(s!("100")).matches(&Value::String(s!("101")), &matcher)).to(be_err());
    expect!(Value::String(s!("100")).matches(&Value::String(s!("10a")), &matcher)).to(be_err());
    expect!(Value::String(s!("100")).matches(&json!(100), &matcher)).to(be_err());
    expect!(Value::String(s!("100")).matches(&json!(100.2), &matcher)).to(be_err());
    expect!(Value::String(s!("100")).matches(&json!("null"), &matcher)).to(be_err());
    expect!(Value::String(s!("100")).matches(&Value::Null, &matcher)).to(be_ok());
  }

  #[test]
  fn compare_maps_handles_wildcard_matchers() {
    let val1 = request!(r#"
    {
      "articles": [
        {
          "variants": {
            "001": {
              "bundles": {
                "001-A": {
                  "description": "someDescription",
                  "referencedArticles": [
                    {
                        "bundleId": "someId"
                    }
                  ]
                }
              }
            }
          }
        }
      ]
    }"#);
    let val2 = request!(r#"{
      "articles": [
        {
          "variants": {
            "002": {
              "bundles": {
                "002-A": {
                  "description": "someDescription",
                  "referencedArticles": [
                    {
                        "bundleId": "someId"
                    }
                  ]
                }
              }
            }
          }
        }
      ]
    }"#);

    let result = match_json(&val1, &val2, &MatchingContext::new(DiffConfig::AllowUnexpectedKeys, &matchingrules!{
      "body" => {
        "$.articles[*].variants.*" => [ MatchingRule::Type ],
        "$.articles[*].variants.*.bundles.*" => [ MatchingRule::Type ]
      }
    }.rules_for_category("body").unwrap()));
    expect!(result).to(be_ok());
  }

  #[test]
  fn compare_lists_with_array_contains_matcher() {
    let val1 = request!(r#"
    [1, 2, 3]
    "#);
    let val2 = request!(r#"
    [10, 22, 6, 1, 5, 3, 2]
    "#);

    let result = match_json(&val1, &val2, &MatchingContext::new(DiffConfig::AllowUnexpectedKeys, &matchingrules!{
      "body" => {
        "$" => [ MatchingRule::ArrayContains(vec![]) ]
      }
    }.rules_for_category("body").unwrap()));
    expect!(result).to(be_ok());
  }

  #[test]
  fn compare_lists_without_array_contains_matcher_fails() {
    let val1 = request!(r#"
    [1, 2, 3]
    "#);
    let val2 = request!(r#"
    [10, 22, 6, 1, 5, 3, 2]
    "#);

    let result = match_json(&val1, &val2, &MatchingContext::with_config(DiffConfig::AllowUnexpectedKeys));
    expect!(result).to(be_err());
  }

  #[test]
  fn compare_lists_with_array_contains_matcher_fails() {
    let val1 = request!(r#"
    [1, 2, 3]
    "#);
    let val2 = request!(r#"
    [10, 22, 6, 1, 5, 2]
    "#);

    let result = match_json(&val1, &val2, &MatchingContext::new(DiffConfig::AllowUnexpectedKeys, &matchingrules!{
      "body" => {
        "$" => [ MatchingRule::ArrayContains(vec![]) ]
      }
    }.rules_for_category("body").unwrap()));
    expect!(result).to(be_err().value(vec![
      BodyMismatch {
        path: "$".to_string(),
        expected: Some("3".into()),
        actual: Some("[\"10\",\"22\",\"6\",\"1\",\"5\",\"2\"]".into()),
        mismatch: "Variant at index 2 (3) was not found in the actual list".to_string()
      }
    ]));
  }

  #[test]
  fn compare_lists_with_array_contains_matcher_with_more_complex_object() {
    let expected = request!(r#"
    {
      "class": [ "order" ],
      "properties": {
          "orderNumber": 42,
          "itemCount": 3,
          "status": "pending"
      },
      "entities": [
        {
          "class": [ "info", "customer" ],
          "properties": {
            "customerId": "pj123",
            "name": "Peter Joseph"
          }
        }
      ],
      "actions": [
        {
          "name": "add-item",
          "title": "Add Item",
          "method": "POST",
          "href": "http://api.x.io/orders/42/items"
        }
      ],
      "links": [
        { "rel": [ "next" ], "href": "http://api.x.io/orders/43" }
      ]
    }
    "#);
    let actual = request!(r#"
    {
      "class": [ "order" ],
      "properties": {
          "orderNumber": 12,
          "itemCount": 6,
          "status": "pending"
      },
      "entities": [
        {
          "class": [ "items", "collection" ],
          "rel": [ "http://x.io/rels/order-items" ],
          "href": "http://api.x.io/orders/12/items"
        },
        {
          "class": [ "info", "customer" ],
          "rel": [ "http://x.io/rels/customer" ],
          "properties": {
            "customerId": "rh565421",
            "name": "Ron Haich"
          },
          "links": [
            { "rel": [ "self" ], "href": "http://api.x.io/customers/rh565421" }
          ]
        }
      ],
      "actions": [
        {
          "name": "add-item",
          "title": "Add Item",
          "method": "POST",
          "href": "http://api.x.io/orders/12/items",
          "type": "application/x-www-form-urlencoded",
          "fields": [
            { "name": "orderNumber", "type": "hidden", "value": "12" },
            { "name": "productCode", "type": "text" },
            { "name": "quantity", "type": "number" }
          ]
        },
        {
          "name": "delete-order",
          "title": "Delete Order",
          "method": "DELETE",
          "href": "http://api.x.io/orders/12"
        },
        {
          "name": "update-order",
          "title": "Update Order",
          "method": "POST",
          "href": "http://api.x.io/orders/12"
        }
      ],
      "links": [
        { "rel": [ "self" ], "href": "http://api.x.io/orders/12" },
        { "rel": [ "previous" ], "href": "http://api.x.io/orders/11" },
        { "rel": [ "next" ], "href": "http://api.x.io/orders/13" }
      ]
    }
    "#);

    let context = MatchingContext::new(DiffConfig::AllowUnexpectedKeys, &matchingrules! {
      "body" => {
        "$.entities" => [
          MatchingRule::ArrayContains(vec![(0, matchingrules_list! {
            "body";
            "$.properties.customerId" => [ MatchingRule::Type ], "$.properties.name" => [ MatchingRule::Type ]
          })])
        ],
        "$.properties.orderNumber" => [ MatchingRule::Integer ],
        "$.properties.itemCount" => [ MatchingRule::Integer ],
        "$.actions" => [
          MatchingRule::ArrayContains(vec![(0, matchingrules_list! {
            "body";
            "$.href" => [ MatchingRule::Regex(".*/orders/\\d+/items".to_string()) ]
          })])
        ],
        "$.links" => [
          MatchingRule::ArrayContains(vec![(0, matchingrules_list! {
            "body";
            "$.href" => [ MatchingRule::Regex(".*/orders/\\d+".to_string()) ]
          })])
        ]
      }
    }.rules_for_category("body").unwrap());
    let result = match_json(&expected, &actual, &context);
    expect!(result).to(be_ok());
  }
}
