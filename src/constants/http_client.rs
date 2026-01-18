/// Default HTTP client connection timeout in seconds.
/// Maximum time to wait for establishing a connection.
pub const DEFAULT_HTTP_CLIENT_CONNECT_TIMEOUT_SECONDS: u64 = 2;

/// Default HTTP client timeout in seconds for CDP service.
/// Overall timeout for HTTP requests in CDP service.
pub const DEFAULT_HTTP_CLIENT_TIMEOUT_SECONDS: u64 = 10;

/// Default maximum number of idle connections per host in the connection pool.
pub const DEFAULT_HTTP_CLIENT_POOL_MAX_IDLE_PER_HOST: usize = 25;

/// Default HTTP client pool idle timeout in seconds.
/// Time after which idle connections are closed.
pub const DEFAULT_HTTP_CLIENT_POOL_IDLE_TIMEOUT_SECONDS: u64 = 30;

/// Default TCP keepalive interval in seconds.
pub const DEFAULT_HTTP_CLIENT_TCP_KEEPALIVE_SECONDS: u64 = 30;

/// Default HTTP/2 keep-alive interval in seconds.
pub const DEFAULT_HTTP_CLIENT_HTTP2_KEEP_ALIVE_INTERVAL_SECONDS: u64 = 30;

/// Default HTTP/2 keep-alive timeout in seconds.
pub const DEFAULT_HTTP_CLIENT_HTTP2_KEEP_ALIVE_TIMEOUT_SECONDS: u64 = 10;
