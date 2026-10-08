# Evidence and Avila Core receipts

Press 5 to open the Evidence step. It shows what Avila Core checked.

## Three separate lines

Compilation, run readiness and the scientific verdict are separate lines. The scientific verdict reads NOT_EVALUATED. It stays that way because the transport uses cold-data surrogate materials and no qualification has been evaluated. [Scope and limits](scope.md) lists what would change that.

## Receipts

Studies run through Avila Core, which records hash-bound receipts. Each receipt binds a result to its exact inputs.

A saved Core workflow reads "executed and verified" when its receipts were rechecked on opening. That states the workflow ran. It does not state that the design works.

In the recorded package the saved receipts live in one evidence store (the `evidence-store` folder, format `avila.core/evidence-store/v0.1`). It keeps each distinct file once, compressed, under its SHA-256. FARIS reads it in place and checks each file's length and SHA-256 as it reads, so a changed, missing or extra file is refused with the file named. `faris evidence inspect --store DIR --case-tree NAME --workspace-tree NAME` shows the same inspection from the command line, and `faris evidence verify-store DIR` checks every file of the store.

If the saved receipts cover only the loaded assumptions, a badge reads "receipts cover the loaded assumptions". Choose **Use the covered assumptions** to switch to them. The receipts do not cover other presets or edited values.

## Receipts not included

By default a `.faris` file records the Core evidence archives by name and hash and does not store them. The study opens fully. The Evidence step then says "Core receipts not included", with the reason and the next step. [Study files (.faris)](study-files.md) explains how to supply the archives.

## Compile and run

**Compile study** in the top bar and **Run bound study stages** need an Avila Core executable. Choose it under **Compiler settings**, or start the app with `--core`. The recorded package supplies one.

Compilation, run readiness and scientific assessment stay separate lines here too. A successful compile or run is not a scientific verdict.

Next: [Study files (.faris)](study-files.md).
