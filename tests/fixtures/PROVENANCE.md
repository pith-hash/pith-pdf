
## Standalone-suite inheritance

These fixtures were inherited byte-exact (sha256-verified at port time)
from the `modhash-pdf` crate of the upstream `modhash` monorepo, where
`tools/gen_fixtures.py` generated them. The generator is not re-shipped
here; the files above are the committed truth this suite tests against,
and `reference.json` at the repository root pins every extraction output.
