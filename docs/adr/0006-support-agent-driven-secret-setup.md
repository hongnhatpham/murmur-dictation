# Support agent-driven secret setup

The deterministic setup CLI allows an explicitly authorized AI agent to configure provider API keys, accepts secrets through standard input, redacts them from output, and writes them directly to Windows Credential Manager. Keys must never appear in command arguments, configuration files, logs, or Git because hands-off setup does not justify leaving durable secret copies.
