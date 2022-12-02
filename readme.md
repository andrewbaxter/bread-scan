This is a command line tool for scanning repository dependencies to generate a Bread `.bread.yml` file.

Supported dependency files:

- Rust, `Cargo.toml`
- Golang, `go.mod`

# Installation

`cargo install bread-scan`

# Usage

`bread-scan`

This will scan and generate a new `.bread.yml` file, replacing one if present.

Commit and push this file and your project is ready to accept (and redistribute) donations.
