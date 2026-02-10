#!/bin/sh
set -eu

endpoint="http://localstack:4566"
account="000000000000"

create_pair() {
  queue_name="$1"
  visibility="$2"
  max_receive="$3"

  dlq_name="${queue_name}-dlq.fifo"
  main_name="${queue_name}.fifo"

  # Note: For production AWS, add DeduplicationScope=messageGroup,FifoThroughputLimit=perMessageGroupId
  # to enable high-throughput FIFO (70k msg/s). Omitted here for LocalStack compatibility.
  aws --endpoint-url "$endpoint" sqs create-queue \
    --queue-name "$dlq_name" \
    --attributes FifoQueue=true,ContentBasedDeduplication=false >/dev/null

  dlq_url="${endpoint}/${account}/${dlq_name}"
  dlq_arn="$(aws --endpoint-url "$endpoint" sqs get-queue-attributes \
    --queue-url "$dlq_url" \
    --attribute-names QueueArn \
    --query "Attributes.QueueArn" \
    --output text)"

  aws --endpoint-url "$endpoint" sqs create-queue \
    --queue-name "$main_name" \
    --attributes "FifoQueue=true,ContentBasedDeduplication=false,VisibilityTimeout=${visibility}" >/dev/null

  main_url="${endpoint}/${account}/${main_name}"
  # --attributes uses JSON format (outer braces) because the RedrivePolicy value
  # is itself JSON, which breaks the shorthand Key=Value parser.
  aws --endpoint-url "$endpoint" sqs set-queue-attributes \
    --queue-url "$main_url" \
    --attributes '{"RedrivePolicy":"{\"deadLetterTargetArn\":\"'"$dlq_arn"'\",\"maxReceiveCount\":\"'"$max_receive"'\"}"}' >/dev/null
}

create_pair "relayer-transaction-request" 30 6
create_pair "relayer-transaction-submission" 30 2
create_pair "relayer-status-check" 30 1000
create_pair "relayer-status-check-evm" 30 1000
create_pair "relayer-status-check-stellar" 20 1000
create_pair "relayer-notification" 60 6
create_pair "relayer-token-swap-request" 60 3
create_pair "relayer-relayer-health-check" 60 3

echo "SQS queues initialized"
