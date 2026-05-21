import { PluginContext } from "../lib/plugin";

type EchoParams = {
    force_error?: boolean;
    [key: string]: unknown;
};

export async function handler(context: PluginContext): Promise<Record<string, unknown>> {
    const { params, route, method, query, headers, config } = context;
    const typedParams = (params ?? {}) as EchoParams;

    console.log("e2e-echo invoked", { route, method });

    if (typedParams.force_error) {
        throw new Error("forced error from e2e-echo plugin");
    }

    return {
        plugin: "e2e-echo",
        route,
        method,
        query,
        params: typedParams,
        config: config ?? null,
        has_authorization_header: Boolean(headers?.authorization?.length),
    };
}
