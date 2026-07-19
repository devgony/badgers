# python-sample

Sample Python package used to exercise `badgers collect python` in CI.

```bash
python -m coverage run -m unittest discover
badgers collect python --repo-root . -o coverage-snapshot.json
```
