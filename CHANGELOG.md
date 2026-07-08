# Changelog

## [0.2.0] - 2026-07-07

### Changed
- Invalid Unicode decimal escapes such as `\2` or `\8` now return a parse error for inputs that 0.1.0 analyzed. (#17)
- Character classes with reversed ranges such as `[z-a]` now return a parse error for inputs that 0.1.0 analyzed. (#18)

### Performance
- Atomic group analysis avoids duplicate set comparison work. (#19)

## [0.2.0] - 2026-07-07

### Changed
- Invalid Unicode decimal escapes such as `\2` or `\8` now return a parse error for inputs that 0.1.0 analyzed. (#17)
- Character classes with reversed ranges such as `[z-a]` now return a parse error for inputs that 0.1.0 analyzed. (#18)

### Performance
- Atomic group analysis avoids duplicate set comparison work. (#19)
