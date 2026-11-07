# ud-mdtoc

Generate a Markdown table of contents from headings, or update one between
`<!-- toc -->` and `<!-- tocstop -->` markers. Package
`@uniquedivine/ud-mdtoc` installs command `ud-mdtoc` and supports the `ud`
executable plugin interface.

Requires Bun 1.2 or later on Linux, macOS, or WSL. The executable uses
`env -S bun --no-install`, so missing dependencies fail instead of loading
packages from Bun's runtime auto-install cache.

## Install

```sh
bun add --global @uniquedivine/ud-mdtoc@0.1.0
ud-mdtoc --version
ud-mdtoc --help
```

The package ships TypeScript source and declares its runtime dependencies.
Installing from npm installs those dependencies independently of a Jiyuu checkout.
The separate upstream command `markdown-toc` can remain installed alongside it.

## Generate a Table of Contents

```sh
ud-mdtoc notes.md
ud-mdtoc 'notes with spaces.md' --bullets=- --maxdepth=3 --no-firsth1
printf '# Title\n\n## First\n\n### Second\n' |
  ud-mdtoc - --bullets=- --maxdepth=3 --no-firsth1
```

The last command prints:

```markdown
- [First](#first)
  - [Second](#second)
```

Without `--bullets`, the list markers rotate through `-`, `*`, and `+` by
depth. `--maxdepth` limits the absolute heading level. `--no-firsth1` omits
the initial level-one title. Headings inside fenced code blocks are ignored.

## Update a File

Place an opening marker before the headings to list:

```markdown
# Notes

<!-- toc -->

## First

### Second
```

Then run:

```sh
ud-mdtoc notes.md -i --bullets=- --maxdepth=3 --no-firsth1
```

The command inserts the TOC and a closing `<!-- tocstop -->` marker. Later runs
replace the content between the markers. Only one TOC is supported per file.
The command writes the input file directly, so save editor changes first.

For Neovim, commands `:Toc` and `:TocCopy` are configured in the owner's dotfiles.
They use the installed command with these same bullet and heading flags.

## Options

| Option                        | Behavior                                         |
| ----------------------------- | ------------------------------------------------ |
| `-i`                          | Update the input file at its TOC marker          |
| `--json`                      | Print parsed heading metadata as JSON            |
| `--bullets=-`                 | Use one marker at every depth                    |
| `--bullets '*' --bullets '-'` | Rotate the specified markers by depth            |
| `--maxdepth=3`                | Render heading levels one through three          |
| `--no-firsth1`                | Omit the first level-one heading                 |
| `--no-stripHeadingTags`       | Preserve HTML tags when generating heading slugs |
| `--indent='    '`             | Use four spaces per nesting level                |
| `--append=TEXT`               | Append text to the rendered TOC                  |
| `-h`, `--help`                | Show usage                                       |
| `-v`, `--version`             | Show the installed version                       |
| `--plugin-info`               | Print the `ud` plugin metadata as JSON           |

Use `-` as the input filename to read stdin. In-place mode requires a file and
cannot be combined with `--json`. JSON exposes parsed heading metadata; the
rendered TOC's depth limit does not filter that metadata.

## Use as a ud Plugin

The `ud` dispatcher discovers executable plugins in its XDG plugin directory,
not on the general `PATH`. Register the installed command:

```sh
plugin_dir="${XDG_DATA_HOME:-$HOME/.local/share}/ud/plugins"
mkdir -p "$plugin_dir"
ln -sfn "$(bun pm bin --global)/ud-mdtoc" "$plugin_dir/ud-mdtoc"
ud plugin info mdtoc
ud mdtoc --help
ud mdtoc notes.md --bullets=- --maxdepth=3 --no-firsth1
```

The owner's dotfiles restore this link through `just sync` and inspect it through
`just health`.

## Develop and Validate

From this directory:

```sh
just install
just test
just typecheck
just lint
just smoke-test
just validate
```

`just smoke-test` packs the publication files, installs the tarball into a
disposable Bun global directory, and runs the installed command outside the
checkout. It checks metadata, stdin, filenames containing spaces, repeated
in-place updates, and failure after deleting a runtime dependency. It does not
replace the workstation's installed package.

`just validate` runs the tests, type checking, package lint, tarball smoke test,
and npm publication dry run. Type checking uses native `bun check`.

## Publish

Publishing follows the `tg-sai` workflow with pinned `npm@12.2.0`. Commands use
Bun, Just, `jq`, and `rg`. Run the checks before the publication command:

```sh
just validate
bunx npm@12.2.0 login --auth-type=web
just publish
```

`just publish` verifies npm login, rejects a version that is already published,
and repeats the npm publication dry run. It then prints the browser
confirmation URL for the owner to complete 2FA. Publishing is explicit and is
never part of `just sync`.

For later releases, edit the version in `package.json`, run `just validate`,
commit the release changes, and run `just publish`. After publication, update
the exact version in the owner's dotfiles Bun manifest and refresh its lockfile.

## Attribution

This project is a TypeScript fork of
[Jon Schlinkert's markdown-toc](https://github.com/jonschlinkert/markdown-toc).
It retains the original contributors in `package.json` and the MIT license in
[LICENSE](LICENSE).
