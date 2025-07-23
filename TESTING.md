# sret Testing Suite

This document describes the comprehensive testing suite for sret.

## Test Structure

The test suite is organized into three main categories:

### 1. Unit Tests

Located in the source code modules, these tests verify individual components.

### 2. Integration Tests

Located in `tests/integration_tests.rs`, these tests verify end-to-end functionality.

### 3. Performance Tests

Located in `tests/performance_tests.rs`, these tests measure proxy performance.

## Running Tests

### Run All Tests

```bash
cargo test
```

### Run Specific Test Categories

```bash
# Unit tests only
cargo test --lib

# Integration tests only
cargo test --test integration_tests

# Performance tests only
cargo test --test performance_tests
```

### Run Individual Tests

```bash
# Run a specific integration test
cargo test test_domain_based_routing

# Run a specific performance test
cargo test test_concurrent_requests
```

## Test Features

### Mock Servers

Simulate backends with concurrent handling and varied responses.

### Load Balancing Verification

Validate Round Robin, Least Connections, Random, and Weighted strategies.

### Routing Logic Testing

Test domain/path matching, precedence, and strict fallback behavior.

### Performance Benchmarks

Measure latency, throughput, routing speed, and error handling.

### Health Check Testing

Verify health tracking, exclusion, intervals, and endpoint access.

## Test Configuration

Tests use separate port ranges to avoid conflicts:

- **Integration Tests**: Ports 18000-18999
- **Performance Tests**: Ports 19000-19999
- **Mock Servers**: Various ranges for different test scenarios

## Expected Performance Characteristics

Based on the performance tests, SRET demonstrates:

- **Individual Request Latency**: < 50ms for routing decisions
- **Concurrent Handling**: 20+ simultaneous requests efficiently
- **Error Response Time**: < 30ms for 404 responses
- **Load Balancer Overhead**: < 100ms additional latency
- **Memory Usage**: Efficient with atomic operations and Arc-based sharing
