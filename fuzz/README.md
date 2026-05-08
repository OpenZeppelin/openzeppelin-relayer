# Fuzz testing with cargo-fuzz (Experimental)

## Setup

Follow the setup instructions in the [cargo-fuzz documentation](https://rust-fuzz.github.io/book/cargo-fuzz/setup.html)

## Running Fuzz Tests

To run the fuzz tests, use the following command:

* for the `extract_public_key_from_der` target:

  ```bash
  cargo +nightly fuzz run extract_public_key_from_der
  ```

## Limitations

* More fuzz targets need to be added.
* It would be much more efficient to run [structure-aware fuzzing](https://rust-fuzz.github.io/book/cargo-fuzz/structure-aware-fuzzing.html), which requires a custom corpus.

## References

* [https://github.com/rust-fuzz/cargo-fuzz]
* [https://rust-fuzz.github.io/book/cargo-fuzz.html]
