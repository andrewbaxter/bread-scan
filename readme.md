This is a command line tool for scanning dependencies for Bread donations. You can use it to update personal donations or to generate a [Bread](https://bre.ad) `.bread.yml` file for a project.

Supported dependency files:

- Javascript, `package.json` (requires a populated `node_modules` directory for metadata)
- Python, `pyproject.toml` (Poetry only)
- Go, `go.mod`
- Rust, `Cargo.toml`
- Java, `pom.xml`

Supported operating systems for scanning:

- Arch
- Debian

# Installation

`cargo install bread-scan`

# Usage - donations

You'll need to set up a token at <https://bre.ad/tokens> with config read and write permission, and put it in an environment variable named `BREAD_TOKEN`.

Run: `bread-scan -s os=debian -d donate`

This scans your system for manually installed packages and tries to figure out the corresponding repository, then updates your donation configuration with them.

The results will be merged with your existing donation targets, keeping existing weights. If you want to remove software you're no longer using, use `--remove`.

# Usage - project yaml

If you're generating a yaml file for your project, you can use the following invocation.

Run: `bread-scan -s project=. -d project_yaml=.`

`bread-scan` will look for dependencies in the project at the current directory using common dependency management systems (`npm`, `cargo`, etc).

This will merge with an existing yaml file if it exists, preserving existing weights. It will keep projects even if it didn't find them during a scan. You can use `--remove` to have it remove them (or similarly, `--remove-accounts` for accounts).

Commit and push this file and your project is ready to accept (and redistribute) donations!
