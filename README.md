# sret - Rust Reverse Proxy

sret is a high-performance Layer 7 reverse proxy written in Rust, featuring load balancing and configurable health checks.

## Features

- **Layer 7 Reverse Proxy**: HTTP traffic forwarding
- **Load Balancing**: Multiple algorithms (Round Robin, Least Connections, Random)
- **Domain-Based Routing**: Route requests to different upstreams based on Host header
- **Health Checks**: Automatic upstream health monitoring
- **Configuration-Driven**: YAML-based configuration
- **High Performance**: Built with Tokio for async performance
- **Monitoring**: Built-in logging and metrics

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
proxy:
  bind_address: "0.0.0.0"
  port: 80

load_balancer:
  strategy: "RoundRobin" # Options: RoundRobin, LeastConnections, Random
  health_check:
    interval_seconds: 30
    timeout_seconds: 5
    expected_status: 200

upstreams:
  - id: "backend1"
    host: "127.0.0.1"
    port: 3000
    weight: 1
    health_check_path: "/health"

  - id: "backend2"
    host: "127.0.0.1"
    port: 3001
    weight: 1
    health_check_path: "/health"

  - id: "backend3"
    host: "127.0.0.1"
    port: 3002
    weight: 2
    health_check_path: "/health"

routes:
  - domain: "service-1.home.arpa"
    upstream_ids: ["backend1"]
  - domain: "service-2.home.arpa"
    upstream_ids: ["backend2", "backend3"]
    load_balancer_strategy: "LeastConnections"
```

### Domain-Based Routing

sret supports routing requests to different upstream servers based on the requested domain (Host header). This allows you to:

- Route `service-1.home.arpa` → `localhost:3000`
- Route `service-2.home.arpa` → Load balance between multiple backends
- Fallback to default load balancer for unmatched domains

Each route can specify its own load balancing strategy, or inherit from the global configuration.

## Load Balancing Algorithms

### Round Robin

Distributes requests evenly across all available upstreams in a rotating fashion.

```yaml
load_balancer:
  strategy: "RoundRobin"
```

### Least Connections

Routes requests to the upstream with the fewest active connections.

```yaml
load_balancer:
  strategy: "LeastConnections"
```

### Random

Randomly selects from available upstreams.

```yaml
load_balancer:
  strategy: "Random"
```

## Health Checks

sret supports automatic health checking of upstream servers:

```yaml
load_balancer:
  health_check:
    interval_seconds: 30 # Check every 30 seconds
    timeout_seconds: 5 # 5 second timeout per check
    expected_status: 200 # Expected HTTP status code
```

When health checks are enabled:

- Unhealthy upstreams are automatically removed from the load balancer pool
- Healthy upstreams are automatically re-added when they recover
- Health check failures are logged for monitoring

## Running Tests

### Unit Tests

```bash
cargo test --lib
```

### Integration Tests

```bash
cargo test --test integration_tests
```

### Performance Tests

```bash
cargo test --test performance_tests
```

### All Tests

```bash
cargo test
```

## Example Usage

### Basic HTTP Proxy

1. Start your backend servers:

```bash
# Terminal 1
python3 -m http.server 3000

# Terminal 2
python3 -m http.server 3001
```

2. Configure sret:

```yaml
proxy:
  bind_address: "127.0.0.1"
  port: 8080

load_balancer:
  strategy: "RoundRobin"

upstreams:
  - id: "backend1"
    host: "127.0.0.1"
    port: 3000
    weight: 1
  - id: "backend2"
    host: "127.0.0.1"
    port: 3001
    weight: 1
```

3. Start sret:

```bash
./target/release/sret --config config.yaml
```

4. Test the proxy:

```bash
curl http://127.0.0.1:8080/
```

## License

This project is licensed under the GPL-3.0 License - see the [LICENSE](LICENSE) file for details.
