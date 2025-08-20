# Midnight Relayer CURL Examples

## Authentication

All requests require the Authorization header with a Bearer token:

```bash
-H "AUTHORIZATION: Bearer b5981311-f311-495c-a124-44086c219bce"
```

## 1. List All Relayers

```bash
curl -X GET http://localhost:8080/api/v1/relayers \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer b5981311-f311-495c-a124-44086c219bce" | jq
```

## 2. Get Specific Midnight Relayer

```bash
curl -X GET http://localhost:8080/api/v1/relayers/midnight-testnet-example \
  -H "AUTHORIZATION: Bearer b5981311-f311-495c-a124-44086c219bce" | jq
```

## 3. Get Relayer Balance

```bash
curl -X GET http://localhost:8080/api/v1/relayers/midnight-testnet-example/balance \
  -H "AUTHORIZATION: Bearer b5981311-f311-495c-a124-44086c219bce" | jq
```

## 4. Submit a Midnight Transaction

```bash
curl -X POST http://localhost:8080/api/v1/relayers/midnight-testnet-example/transactions \
  -H "Content-Type: application/json" \
  -H "AUTHORIZATION: Bearer b5981311-f311-495c-a124-44086c219bce" \
  -d '{
    "guaranteed_offer": {
      "inputs": [
        {
          "origin": "78ad774b434f1c560eca9d2b6fbf02f16f5e21c1d3d7f79a93a30b686f30c188",
          "token_type": "02000000000000000000000000000000000000000000000000000000000000000000",
          "value": "1000000"
        }
      ],
      "outputs": [
        {
          "destination": "8e0622a9987a7bef7b6a1417c693172b79e75f2308fe3ae9cc897f6108e3a067",
          "token_type": "02000000000000000000000000000000000000000000000000000000000000000000",
          "value": "1000000"
        }
      ]
    },
    "intents": [],
    "fallible_offers": []
  }' | jq
```

### Transaction Fields Explanation:

- **origin**: Hex-encoded wallet seed of the sender (32 bytes)
- **destination**: Hex-encoded wallet seed of the recipient (32 bytes)
- **token_type**: Token identifier (tDUST = "02" followed by zeros)
- **value**: Amount in smallest units (speck)
- **guaranteed_offer**: Used for simple transfers that must succeed
- **intents**: For complex contract interactions (empty array for simple transfers)
- **fallible_offers**: For operations that may fail independently (empty array for simple transfers)

## 5. Check Transaction Status

```bash
# Replace {transaction_id} with the actual transaction ID from the submission response
curl -X GET http://localhost:8080/api/v1/relayers/midnight-testnet-example/transactions/{transaction_id} \
  -H "AUTHORIZATION: Bearer b5981311-f311-495c-a124-44086c219bce" | jq
```

Example:

```bash
curl -X GET http://localhost:8080/api/v1/relayers/midnight-testnet-example/transactions/a6a468d1-3e87-48fe-9183-e44a9cb92be7 \
  -H "AUTHORIZATION: Bearer b5981311-f311-495c-a124-44086c219bce" | jq
```

## 6. List Recent Transactions

```bash
curl -X GET "http://localhost:8080/api/v1/relayers/midnight-testnet-example/transactions?per_page=5" \
  -H "AUTHORIZATION: Bearer b5981311-f311-495c-a124-44086c219bce" | jq
```

## 7. List Transactions with Status Filter

```bash
# Show only transaction IDs and statuses
curl -X GET "http://localhost:8080/api/v1/relayers/midnight-testnet-example/transactions?per_page=5" \
  -H "AUTHORIZATION: Bearer b5981311-f311-495c-a124-44086c219bce" | jq '.data[] | {id: .id, status: .status, created_at: .created_at}'
```

## 8. Get Signer Information

```bash
curl -X GET http://localhost:8080/api/v1/signers/local-signer \
  -H "AUTHORIZATION: Bearer b5981311-f311-495c-a124-44086c219bce" | jq
```

## Response Examples

### Successful Transaction Submission Response:

```json
{
  "success": true,
  "data": {
    "id": "a6a468d1-3e87-48fe-9183-e44a9cb92be7",
    "hash": null,
    "pallet_hash": null,
    "block_hash": null,
    "status": "pending",
    "created_at": "2025-08-20T19:11:23.868603+00:00",
    "sent_at": null,
    "confirmed_at": null
  },
  "error": null
}
```

### Transaction Status Values:

- `pending`: Transaction is queued for processing
- `confirmed`: Transaction has been confirmed on the network
- `failed`: Transaction failed to process

### Balance Response:

```json
{
  "success": true,
  "data": {
    "balance": 1000000000,
    "unit": "speck"
  },
  "error": null
}
```

## Test Wallet Seeds

For testing purposes, these wallet seeds were used:

- **Relayer wallet seed**: `78ad774b434f1c560eca9d2b6fbf02f16f5e21c1d3d7f79a93a30b686f30c188`
- **Destination wallet seed**: `8e0622a9987a7bef7b6a1417c693172b79e75f2308fe3ae9cc897f6108e3a067`

## Notes

1. The endpoint path is `/api/v1/relayers/{relayer_id}/transactions`, not `/api/v1/transactions`
2. The transaction data should be placed directly in the request body (not wrapped in a "data" field)
3. All wallet seeds must be 32-byte hex-encoded strings (64 characters)
4. The token type for tDUST is always `"02000000000000000000000000000000000000000000000000000000000000000000"`
5. Transaction values are in the smallest unit (speck), where 1 tDUST = 10^9 speck
