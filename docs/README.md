# Antora Documentation

## Pre-requisites

- You need `antora` `site-generator` and `mermaid` extension to generate the documentation.
- You can directly install these dependencies by running `cd docs && npm i --include dev`. If you want to install them manually, you can follow the steps mentioned below.
- Install `antora` locally, you can follow the steps mentioned [here](https://docs.antora.org/antora/latest/install/install-antora/#install-dir), if you already have you can skip this step.
  > Note: If you want to install globally, you can run: <br/> `npm install -g @antora/cli@3.1 @antora/site-generator@3.1 @sntke/antora-mermaid-extension`
- Verify the installation by running `antora --version` or by running `npx antora --version` if you installed it locally.

## Generate Documentation

- To generate documentation locally, run the following command from the project root directory:
  ```sh
  cargo make rust-antora
  ```
- The site will be generated in `docs/build/site/openzeppelin_relayer/<version>/` directory.
- To view the documentation, open the `docs/build/site/openzeppelin_relayer/<version>/index.html` in your browser.
