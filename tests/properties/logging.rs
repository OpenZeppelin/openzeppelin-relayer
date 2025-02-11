//! Property-based tests for logging.
//!
//! These tests verify the behavior of the `compute_rolled_file_path` function,
//! focusing on template variable substitution and output consistency.
//! The tests ensure that the logging system handles template variables correctly
//! and produces consistent, well-formed output across various input combinations.
//!
//!   Refer to `src/logging/mod.rs` for more details.
use openzeppelin_relayer::logging::compute_rolled_file_path;
use proptest::{prelude::*, test_runner::Config};

proptest! {
  // Set the number of cases to 1000
  #![proptest_config(Config {
    cases: 1000, ..Config::default()
  })]

  /// Property test for compute_rolled_file_path when base ends with ".log"
  #[test]
  fn prop_compute_rolled_file_path_with_log_suffix(
    base in ".*[^.]",
    // ensuring non-empty ending character in date
    date in "[0-9]{4}-[0-9]{2}-[0-9]{2}"
  ) {
      let base_with_log = format!("{}{}.log", base, "");
      let result = compute_rolled_file_path(&base_with_log, &date, 1);
      let expected = format!("{}-{}.{}.log", base_with_log.strip_suffix(".log").unwrap(), date, 1);
      prop_assert_eq!(result, expected);
    }

  /// Property test for compute_rolled_file_path when base does not end with ".log"
  #[test]
  fn prop_compute_rolled_file_path_without_log_suffix(
    base in ".*",
    date in "[0-9]{4}-[0-9]{2}-[0-9]{2}"
  ) {
      // Ensure base does not end with ".log"
      let base_non_log = if base.ends_with(".log")
      {
        format!("{}x", base)
      } else {
        base
      };
      let result = compute_rolled_file_path(&base_non_log, &date,1);
      let expected = format!("{}-{}.{}.log", base_non_log, date, 1);
      prop_assert_eq!(result, expected);
  }
}
