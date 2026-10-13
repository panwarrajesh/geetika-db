# Examples

* `node/server.mjs` - a small backend using the JavaScript SDK (`sdk/js`). The API key lives only here.
* `react/TodoList.tsx` - a React component that calls that backend. It contains no key and never talks to RajeshDB directly.
* `python/example.py` - the Python SDK in a script.

Never put a RajeshDB API key in frontend code, mobile apps, or a public repository. Use a read-only key where reads are enough.
Create keys in the dashboard (Project -> API keys); each key is bound to one project and one environment.
