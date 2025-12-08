/**
 * Example plugin demonstrating wildcard route routing
 *
 * This plugin shows how to implement custom routing logic using the route parameter.
 *
 * Example API calls:
 * - POST /api/v1/plugins/{plugin_id}/call          -> Default handler (route = "")
 * - POST /api/v1/plugins/{plugin_id}/call/verify   -> Verify endpoint (route = "/verify")
 * - POST /api/v1/plugins/{plugin_id}/call/settle   -> Settle endpoint (route = "/settle")
 * - POST /api/v1/plugins/{plugin_id}/call/status   -> Status endpoint (route = "/status")
 */

export async function handler(context: any) {
  const { route, params, api } = context;

  console.log(`Received request for route: ${route}`);

  // Route based on the route
  switch (route) {
    case '':
    case '/':
      return handleDefault(params, api);

    case '/verify':
      return handleVerify(params, api);

    case '/settle':
      return handleSettle(params, api);

    case '/status':
      return handleStatus(params, api);

    default:
      // Return 404 for unknown routes
      const error: any = new Error(`Unknown route: ${route}`);
      error.status = 404;
      error.code = 'NOT_FOUND';
      throw error;
  }
}

/**
 * Default endpoint handler
 */
function handleDefault(params: any, api: any) {
  console.log('Handling default endpoint');
  return {
    message: 'Welcome to the routing example plugin',
    availableEndpoints: [
      '/verify - Verify a transaction',
      '/settle - Settle a transaction',
      '/status - Get status information'
    ],
    params: params
  };
}

/**
 * Verify endpoint handler
 */
async function handleVerify(params: any, api: any) {
  console.log('Handling verify endpoint');

  if (!params.transactionId) {
    const error: any = new Error('transactionId is required');
    error.status = 400;
    error.code = 'VALIDATION_ERROR';
    error.details = { field: 'transactionId' };
    throw error;
  }

  // Example: Get transaction from relayer
  // const tx = await api.getTransaction(params.transactionId);

  return {
    action: 'verify',
    transactionId: params.transactionId,
    verified: true,
    timestamp: new Date().toISOString()
  };
}

/**
 * Settle endpoint handler
 */
async function handleSettle(params: any, api: any) {
  console.log('Handling settle endpoint');

  if (!params.amount || !params.recipient) {
    const error: any = new Error('amount and recipient are required');
    error.status = 400;
    error.code = 'VALIDATION_ERROR';
    error.details = {
      fields: ['amount', 'recipient']
    };
    throw error;
  }

  // Example: Send a settlement transaction
  // const result = await api.useRelayer(params.relayerId).sendTransaction({
  //   to: params.recipient,
  //   value: params.amount,
  //   data: '0x'
  // });

  return {
    action: 'settle',
    amount: params.amount,
    recipient: params.recipient,
    status: 'pending',
    timestamp: new Date().toISOString()
  };
}

/**
 * Status endpoint handler
 */
function handleStatus(params: any, api: any) {
  console.log('Handling status endpoint');

  return {
    action: 'status',
    service: 'routing-example',
    version: '1.0.0',
    uptime: process.uptime(),
    timestamp: new Date().toISOString()
  };
}
