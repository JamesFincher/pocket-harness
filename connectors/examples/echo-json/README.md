# Echo JSON Connector

This is the smallest practical external connector.

Add it to `pocket-harness.yaml`:

```yaml
connectors:
  default: echo_json
  definitions:
    echo_json:
      type: json
      display_name: Echo JSON
      command: ["python3", "connectors/examples/echo-json/echo_connector.py"]
      cwd: "."
      timeout_seconds: 30
      env: {}
      settings: {}
```

Then run:

```bash
cargo run -- check --health
cargo run -- run --thread main test the connector
```
