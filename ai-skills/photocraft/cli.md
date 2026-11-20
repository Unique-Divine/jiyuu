# CLI and Documents

## Inspect and Discover

Run `photocraft-cli --version` and `photocraft-cli <subcommand> --help` to check the installed interface. The `info` subcommand reports canvas size, color mode, bit depth, and the layer tree as JSON. The `--detail` flag adds more layer information.

```sh
photocraft-cli info input.psd
photocraft-cli commands --json --filter 'blur'
```

The JSON command registry includes command IDs, parameter descriptions, and whether a command needs a document. A command may be reported disabled because the registry is being queried without an open document. Inspect the installed registry for the exact ID and parameters before using an edit; menu labels are not command IDs.

## Edit and Verify

The `run` subcommand opens a file or creates a document, executes commands in order, and saves an output. Each `--params` JSON object applies to the preceding `--cmd`. Results are JSON lines. This example creates a 128 by 128 layered test document:

```sh
photocraft-cli run --new '{"width":128,"height":128}' \
  --cmd layer.new.layer --params '{"name":"Test layer"}' \
  --out /tmp/photocraft-test.pcraft
photocraft-cli info /tmp/photocraft-test.pcraft
```

For an existing image, put the input path after `run` and use a distinct output path. Inspect the saved document, and visually inspect a rendered image when appearance matters. Check file size and dimensions explicitly when a form or delivery target sets limits. A successful command response alone does not establish visual correctness.

## Convert and Batch

```sh
photocraft-cli convert input.psd output.png
photocraft-cli convert input.png output.jpg --quality 85
photocraft-cli batch --help
```

The output extension normally selects the format. JPEG and PNG are flat; PSD and `.pcraft` retain layers. TIFF is flat by default in the installed release; `--tiff-layers` requests layered TIFF. JPEG quality controls encoding, not a guaranteed byte limit. Check the resulting file size and adjust quality or dimensions if required.

The `run` and `convert` subcommands reject replacing their input unless `--in-place` is supplied. Batch accepts an action list and separate input and output directories; see its help for the installed version. For action-file formats and detailed export behavior, consult the upstream [development guide](https://github.com/storytold/photocraft/blob/main/docs/development.md#headless-cli) and compare it with installed help.
