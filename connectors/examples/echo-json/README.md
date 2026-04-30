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

The example reports the capabilities required by the default YAML feature set. If you remove
capabilities from a connector, disable the matching feature in `pocket-harness.yaml` before running
`check --health`.
