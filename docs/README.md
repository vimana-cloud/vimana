# Documentation

## Internal

The mono-repo hosts internal documentation at [`docs.corp.vimana.co`](https://TODO).
You can preview it locally by running `bazel run //docs:dev`.

### How to Write Docs

All markdown files are included in the documentation browser.

Files named `README.md` get special treatment.
They serve as the directory index and should:

- Include just enough high-level information
  to orient a person completely unfamiliar with the directory.
- Provide general information related to all contents (including sub-folders)
  of that directory.
  If the information is too specific, or too general, for the whole sub-tree,
  then it probably belongs elsewhere.
  Put it in any individual `.md` file.
