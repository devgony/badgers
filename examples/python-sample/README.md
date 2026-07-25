# python-sample

Sample Python package used to exercise `badgers collect lcov` in CI.

```bash
python -m coverage run -m unittest discover
mkdir -p coverage && python -m coverage lcov -o coverage/lcov.info
badgers collect lcov --repo-root . -o coverage-snapshot.json
```
