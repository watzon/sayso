# Catalogs

The interface text of Sayso, one folder for each locale. See `docs/i18n.md`.

- `en/` is the source. Every other locale has the same files and the same keys.
- A file is `<surface>.<locale>.yml`, for example `en/overlay.en.yml` and
  `ja/overlay.ja.yml`. The last part of the name before `.yml` is the locale.
- Each file starts with `_version: 1`. Files can share a section (several
  files start with `settings:`), but two files must not have the same key.
- Indent with two spaces. Put each text in double quotes on the same line as
  its key.
- `%{name}` is an argument. Keep each argument of the English text, and keep
  its name. The order can change.
- A key with `one` and `other` (and for some languages `many`) under it is a
  plural. A language without plural forms has only `other`.
- A key that ends in `_mac`, `_windows`, or `_other` has a version for each
  system. Translate each one.
- Do not translate model names, provider names, "Sayso", key names such as
  Option or Ctrl, file names, or text in `code`.
