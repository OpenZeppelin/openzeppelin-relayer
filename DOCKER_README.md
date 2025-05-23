# OpenZeppelin Relayer

> :warning: This software is in alpha. Use in production environments at your own risk.

This relayer service enables interaction with blockchain networks through transaction submissions. It offers multi-chain support and an extensible architecture for adding new chains.

[User Docs](https://docs.openzeppelin.com/relayer/) | [Quickstart](https://docs.openzeppelin.com/relayer/quickstart)

## Pre-requisites

- Docker installed on your machine
- [Sodium](https://doc.libsodium.org/). See [install sodium section](https://github.com/OpenZeppelin/openzeppelin-relayer?tab=readme-ov-file#install-sodium) for more information.
- `.env` file with the required environment variables & `config/config.json` file with the required configuration. See how to set it up in [config files section](https://github.com/OpenZeppelin/openzeppelin-relayer?tab=readme-ov-file#config-files) for more information.
- Create signers and add them to the `config/config.json` file. See how to set it up in [signers section](https://github.com/OpenZeppelin/openzeppelin-relayer?tab=readme-ov-file#creating-a-signer) for more information.
- Configure webook url in `config/config.json` file. See how to set it up in [webhook section](https://github.com/OpenZeppelin/openzeppelin-relayer?tab=readme-ov-file#configure-webhook-url) for more information.
- Configure webhook signing key in `config/config.json` file. See how to set it up in [webhook section](https://github.com/OpenZeppelin/openzeppelin-relayer?tab=readme-ov-file#configure-webhook-signing-key) for more information.
- Configure Api key in `config/config.json` file. See how to set it up in [api key section](https://github.com/OpenZeppelin/openzeppelin-relayer?tab=readme-ov-file#configure-api-key) for more information.
- Redis server running. See how to set it up in [redis section](https://github.com/OpenZeppelin/openzeppelin-relayer?tab=readme-ov-file#starting-redis-manually-without-docker-compose) for more information.

> :warning: Redis is automatically started when using docker compose. If you are not using docker compose, you need to create a dedicated network and start redis manually.

## How to use images pushed to Docker Hub

- These images are automatically pulled when you use docker compose. See [using docker compose](https://github.com/OpenZeppelin/openzeppelin-relayer?tab=readme-ov-file#running-services-with-docker-compose) for more information.

### 1. Pull the image

You can pull the latest image using the following command:

```bash
docker pull openzeppelin/openzeppelin-relayer:latest
```

### 2. Run the image

You can run the image using the following command:

```bash
docker run --env-file .env -d \
  --name relayer \
  --network relayer-net \
  -p 8080:8080 \
  -v ./config:/app/config:ro \
  openzeppelin/openzeppelin-relayer:latest
```

### 3. Access the service

Once the container is running, you can access the service at `http://localhost:8080`.

### 4. Stop the container

You can stop the container using the following command:

```bash
docker stop relayer
```

### 5. Remove the container

You can remove the container using the following command:

```bash
docker rm relayer
```

### 6. Remove the image

You can remove the image using the following command:

```bash
docker rmi openzeppelin/openzeppelin-relayer:latest
```

## Contributing

We welcome contributions to the OpenZeppelin Relayer. Please read our [contributing section](https://github.com/OpenZeppelin/openzeppelin-relayer/?tab=readme-ov-file#contributing) for more information.

## Observability

See the [observability section](https://github.com/OpenZeppelin/openzeppelin-relayer/?tab=readme-ov-file#observability) for more information on how to set up observability for the relayer.

## License

This project is licensed under the GNU Affero General Public License v3.0 - see the [LICENSE](LICENSE) file for details.

## Security

For security concerns, please refer to our [Security Policy](SECURITY.md).
