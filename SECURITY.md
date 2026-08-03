# Security policy

## Supported versions

Until the first stable release, only the current `main` branch receives
security fixes.

## Reporting

Report vulnerabilities through [GitHub private vulnerability reporting](https://github.com/themixednuts/gpui-studio/security/advisories/new).
Do not open a public issue for an MCP authentication bypass, project path
escape, unsafe file replacement, symlink traversal, secret disclosure, or an
unintended action that crosses the selected project/window boundary.

Include the affected commit and operating system, a minimal reproduction, the
expected boundary, and whether credentials or user data were exposed.

## Deployment guidance

- Keep MCP disabled with `--no-mcp` when agent access is not required.
- Treat enabled MCP clients as trusted local automation principals.
- Open only projects whose Rust hooks and native components you trust.
- Keep `.gpui-studio/` project state private and out of source control unless a
  specific durable document is intentionally shared.
- Review macOS Screen Recording and Linux portal permissions before enabling
  capture-dependent workflows.
