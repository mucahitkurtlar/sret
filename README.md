# sret - Rust Reverse Proxy

sret is a high-performance Layer 7 reverse proxy written in Rust, featuring advanced routing, load balancing, and comprehensive health monitoring.

## Features

- **Multi-Server Architecture**: Configure multiple proxy servers with different routing rules
- **HTTPS/TLS Support**: Optional SSL/TLS termination with certificate configuration
- **Advanced Routing**: Domain and path-based routing with flexible matching
- **Load Balancing**: Four algorithms (Round Robin, Least Connections, Random, Weighted Round Robin)
- **Health Monitoring**: Automatic upstream health checking with configurable intervals
- **Path Rewriting**: Automatic path prefix stripping for clean backend requests
- **Configuration-Driven**: YAML-based configuration with validation
- **High Performance**: Built with Tokio and modern async Rust patterns
- **Comprehensive Testing**: Thoroughly tested across unit, integration, and performance scenarios

## Quick Start

### Installation

1. Clone the repository:

```bash
git clone https://github.com/mucahitkurtlar/sret.git
cd sret
```

2. Build the project:

```bash
cargo build --release
```

3. Run with default configuration:

```bash
./target/release/sret --config config.yaml
```

### Configuration

Create a `config.yaml` file:

```yaml
upstreams:
  - id: "backend"
    targets:
      - host: "127.0.0.1"
        port: 8080
        weight: 1
        health_check_path: "/health"
      - host: "127.0.0.1"
        port: 8081
        weight: 2
        health_check_path: "/health"
    strategy: "RoundRobin"
    health_check:
      interval_seconds: 30
      timeout_seconds: 5
      expected_status: 200
  - id: "console"
    targets:
      - host: "127.0.0.1"
        port: 3000
  - id: "ui"
    targets:
      - host: "127.0.0.1"
        port: 3001

servers:
  - id: "main"
    bind_address: "0.0.0.0"
    port: 80
    routes:
      - domains:
          - "api.home.arpa"
          - "service.home.arpa"
        paths:
          - "/api"
          - "/service"
        upstream: "backend"

      - domains:
          - "console.home.arpa"
        paths:
          - "/console"
        upstream: "console"

      - domains:
          - "ui.home.arpa"
          - "dashboard.home.arpa"
        paths:
          - "/ui"
          - "/dashboard"
        upstream: "ui"
```

## Architecture

### Multi-Server Design

sret supports multiple proxy servers, each with their own:

- **Bind Address & Port**: Independent network endpoints
- **Routing Rules**: Domain and path-based request matching
- **Upstream Associations**: Flexible backend server groups

### Advanced Routing

**Domain-Based Routing**: Route requests based on the Host header

```yaml
routes:
  - domains: ["api.example.com", "service.example.com"]
    upstream: "backend"
```

**Path-Based Routing**: Route requests based on URL paths with automatic prefix stripping

```yaml
routes:
  - paths: ["/api", "/v1"]
    upstream: "backend"
    # Request to /api/users becomes /users at backend
```

**Combined Routing**: Use both domain and path matching for precise control

```yaml
routes:
  - domains: ["api.example.com"]
    paths: ["/v1", "/v2"]
    upstream: "backend"
```

### Upstream Management

Each upstream defines a group of target servers with:

- **Load Balancing**: Distribute requests across multiple targets
- **Health Monitoring**: Automatic health checks and failover
- **Weighted Distribution**: Control traffic distribution with target weights

## Load Balancing Algorithms

sret supports four load balancing strategies:

### Round Robin

Distributes requests evenly across all available targets in sequence.

```yaml
strategy: "RoundRobin"
```

### Least Connections

Routes requests to the target with the fewest active connections.

```yaml
strategy: "LeastConnections"
```

### Random

Randomly selects from available healthy targets.

```yaml
strategy: "Random"
```

### Weighted Round Robin

Distributes requests based on target weights, allowing traffic shaping.

```yaml
strategy: "WeightedRoundRobin"
targets:
  - host: "127.0.0.1"
    port: 8080
    weight: 3 # Receives 3x more traffic
  - host: "127.0.0.1"
    port: 8081
    weight: 1 # Baseline traffic
```

## Health Checks

sret supports comprehensive health monitoring with automatic failover:

```yaml
upstreams:
  - id: "backend"
    health_check:
      interval_seconds: 30 # Check every 30 seconds
      timeout_seconds: 5 # 5 second timeout per check
      expected_status: 200 # Expected HTTP status code
    targets:
      - host: "127.0.0.1"
        port: 8080
        health_check_path: "/health" # Per-target health endpoint
```

**Health Check Behavior**:

- Unhealthy targets are automatically excluded from load balancing
- Healthy targets are re-added when they recover
- Health status changes are logged for monitoring
- Each target can have its own health check endpoint

## HTTPS/TLS Support

sret supports HTTPS with TLS termination using SSL certificates:

```yaml
servers:
  - id: "https-server"
    bind_address: "0.0.0.0"
    port: 443
    tls:
      cert_path: "/path/to/certificate.pem"
      key_path: "/path/to/private-key.pem"
    routes:
      - domains: ["secure.example.com"]
        upstream: "backend"
```

**TLS Configuration**:

- **cert_path**: Path to the PEM-encoded certificate file (certificate chain)
- **key_path**: Path to the PEM-encoded private key file
- **Protocol Support**: HTTP/1.1 over TLS
- **Certificate Formats**: PEM format certificates and keys

**Certificate Requirements**:

- Certificates must be in PEM format
- Private keys must be in PEM format (RSA or ECDSA)
- Certificate chain should include intermediate certificates if required
- File paths must be accessible by the sret process

**Mixed HTTP/HTTPS Setup**:

You can run both HTTP and HTTPS servers simultaneously:

```yaml
servers:
  # HTTPS server
  - id: "https"
    bind_address: "0.0.0.0"
    port: 443
    tls:
      cert_path: "/etc/ssl/certs/example.pem"
      key_path: "/etc/ssl/private/example.key"
    routes:
      - domains: ["secure.example.com"]
        upstream: "backend"

  # HTTP server (for redirects or development)
  - id: "http"
    bind_address: "0.0.0.0"
    port: 80
    routes:
      - domains: ["example.com"]
        upstream: "backend"
```

## Testing

For detailed testing information, see [TESTING.md](TESTING.md).

## Example Usage

### Basic Multi-Service Proxy

1. Start your backend services:

```bash
# API Service #1
python3 -m http.server 8080

# API Service #2
python3 -m http.server 8081

# Web UI
python3 -m http.server 3000

# Admin Console
python3 -m http.server 3001
```

2. Create `config.yaml`:

```yaml
upstreams:
  - id: "api"
    targets:
      - host: "127.0.0.1"
        port: 8080
        weight: 1
        health_check_path: "/health"

      - host: "127.0.0.1"
        port: 8081
        weight: 2
        health_check_path: "/health"
    strategy: "WeightedRoundRobin"
    health_check:
      interval_seconds: 30
      timeout_seconds: 5
      expected_status: 200

  - id: "ui"
    targets:
      - host: "127.0.0.1"
        port: 3000

  - id: "admin"
    targets:
      - host: "127.0.0.1"
        port: 3001

servers:
  - id: "main-proxy"
    bind_address: "127.0.0.1"
    port: 80
    routes:
      - domains: ["api.home.arpa"]
        paths: ["/api", "/v1"]
        upstream: "api"

      - domains: ["app.home.arpa"]
        upstream: "ui"

      - paths: ["/admin"]
        upstream: "admin"
```

3. Start sret:

```bash
./target/release/sret --config config.yaml
```

4. Test the routing:

```bash
# Routes to API service
curl -H "Host: api.localhost" http://127.0.0.1/api/users

# Routes to UI service
curl -H "Host: app.localhost" http://127.0.0.1/

# Routes to admin service
curl http://127.0.0.1/admin/dashboard
```

## Configuration Reference

### Complete Configuration Schema

```yaml
upstreams:
  - id: "string" # Unique upstream identifier
    targets: # List of backend servers
      - host: "string" # Target hostname/IP
        port: number # Target port
        weight: number # Optional: Traffic weight (default: 1)
        health_check_path: "string" # Optional: Health check endpoint
    strategy: "string" # Optional: RoundRobin|LeastConnections|Random|WeightedRoundRobin
    health_check: # Optional: Health check configuration
      interval_seconds: number # Check interval (default: 30)
      timeout_seconds: number # Check timeout (default: 5)
      expected_status: number # Expected HTTP status (default: 200)

servers:
  - id: "string" # Unique server identifier
    bind_address: "string" # Bind IP address
    port: number # Listen port
    tls: # Optional: TLS/HTTPS configuration
      cert_path: "string" # Path to PEM certificate file
      key_path: "string" # Path to PEM private key file
    routes: # List of routing rules
      - domains: ["string"] # Optional: Domain matching
        paths: ["string"] # Optional: Path prefix matching
        upstream: "string" # Target upstream ID
```

### Configuration Validation

sret validates configuration on startup and will report:

- Missing required fields
- Invalid upstream references
- Duplicate server IDs
- Invalid load balancing strategies
- Port conflicts

## Command Line Usage

```bash
sret [OPTIONS]

Options:
  -c, --config <FILE>    Configuration file path [default: config.yaml]
  -h, --help             Print help information
```

## Development

### Building from Source

```bash
git clone https://github.com/mucahitkurtlar/sret.git
cd sret
cargo build --release
```

### Running Tests

See [TESTING.md](TESTING.md) for detailed test instructions.

## License

This project is licensed under the GPL-3.0 License - see the [LICENSE](LICENSE) file for details.
