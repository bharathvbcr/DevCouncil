# Hostile path fixtures

A file whose name contains `<` / `>` (e.g. `x<img src=x onerror=alert(1)>.ts`)
is a valid adversarial case on APFS/ext4 but **cannot live in git**: Windows
checkouts reject those characters, which breaks cargo-dist and the kernel
Windows matrix before any test runs.

Hostile *names* are still covered in-process by the extract/query/resolve
adversarial tests, which pass the path as a string without creating the file.
