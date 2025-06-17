# Antora Documentation

## Generate Documentation

- To generate documentation locally, run the following command

  ```sh
  yarn docs:watch
  ```

  - In separate terminal from root of the repo run:

    ```sh
    cargo make rust-antora
    ```

- You can view the site `localhost:8080` or other port if it's in use.

- To update openapi documentation, run:

  ```sh
  cargo run --gen-openapi generate_openapi -- ./docs/openapi.json
  ```
