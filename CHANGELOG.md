# Changelog

## v0.6.0 (April 21, 2023)
- Update to `syn` 2.0

## v0.5.0 (December 20, 2021)
- Implement `std::error::Error::source` for `Error`.
- Documentation fixes.
- Remove hard dependency on proc-macro crate [#19](https://github.com/TedDriggs/syn-inline-mod/pull/19)

## v0.4.0 (July 8, 2020)
- Expose errors for invalid Rust source code ([#11](https://github.com/TedDriggs/syn-inline-mod/issues/11), [#13](https://github.com/TedDriggs/syn-inline-mod/pull/13))

## v0.2.0 (June 18, 2019)
- Distinguish root and non-root modules ([#6](https://github.com/TedDriggs/syn-inline-mod/pull/6))

## v0.1.1 (February 13, 2019)
- Preserve inner attributes from files on inlining ([#1](https://github.com/TedDriggs/syn-inline-mod/issues/1))
