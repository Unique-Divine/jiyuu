# Installation and Versions

The [PhotoCraft repository](https://github.com/storytold/photocraft) documents source builds and links to release packages for each platform. A source clone is not itself an installed application. Check the OS and architecture before choosing a release, and consult its release notes and checksums. A matching release includes both desktop and CLI programs.

```sh
uname -sm
photocraft --version
photocraft-cli --version
photocraft-cli --help
```

On Linux, upstream distributes AppImage, tarball, and other package formats. The AppImage also exposes the CLI through `--cli`; see the release's README for its desktop integration behavior. A source build is available through Cargo, but building the desktop app can take substantial time and disk space.

Check release-specific documentation and help rather than assuming the latest source matches an installed binary. For example, v0.23.1 rejects the `mcp --headless` flag documented in newer source. Its bundled `share/doc/photocraft/mcp.md` describes the release's headless behavior. See [Agent Automation](automation.md) for the practical distinction.

If the CLI works but the desktop app does not open, check the graphical display and the app's stderr. Headless CLI operations do not require a desktop window. To determine whether a launched app remains running, check its process and log; a live process alone does not prove the window or document rendered correctly.
