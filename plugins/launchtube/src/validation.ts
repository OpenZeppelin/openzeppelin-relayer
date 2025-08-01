import { LaunchtubeRequest, ValidationError } from './types';

export function validateAndParseRequest(params: any): LaunchtubeRequest {
  // First, validate the basic parameter structure
  if (params.sim === false && !params.xdr) {
    throw new ValidationError('Cannot pass `sim = false` without `xdr`');
  }

  if (!params.xdr && !params.func && !params.auth) {
    throw new ValidationError('Must pass either `xdr` or `func` and `auth`');
  }

  if (params.xdr && (params.func || params.auth)) {
    throw new ValidationError('`func` and `auth` must be omitted when passing `xdr`');
  }

  if (!params.xdr && !(params.func && params.auth)) {
    throw new ValidationError('`func` and `auth` are both required when omitting `xdr`');
  }

  // Determine request type and return structured request
  const sim = params.sim !== false; // Default to true if not specified

  if (params.xdr) {
    return {
      type: 'xdr',
      xdr: params.xdr,
      sim,
    };
  } else {
    return {
      type: 'func-auth',
      func: params.func,
      auth: params.auth,
      sim,
    };
  }
}
