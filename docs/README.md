# How to Write Docs

Run a local documentation server.
All markdown files in the repo will be rendered.

```bash
bazel run //docs:dev
```

Files named `README.md` get special treatment.
They serve as the directory index and should:

- Include just enough high-level information
  to orient a person completely unfamiliar with the directory.
- Provide general information related to all contents (including sub-folders)
  of that directory.
  If the information is too specific, or too general, for the whole sub-tree,
  then it probably belongs elsewhere.
  Put it in any individual `.md` file.
