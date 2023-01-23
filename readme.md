This is a command line tool for scanning repository dependencies to generate a [Bread](https://bre.ad) `.bread.yml` file.

Supported dependency files:

- Javascript, `package.json` (requires a populated `node_modules` directory for metadata)
- Python, `pyproject.toml` (Poetry only)
- Go, `go.mod`
- Rust, `Cargo.toml`

# Installation

`cargo install bread-scan`

# Usage

`bread-scan`

This will scan and generate a new `.bread.yml` file or replace the projects in an existing `.bread.yml` if present.

Commit and push this file and your project is ready to accept (and redistribute) donations.
