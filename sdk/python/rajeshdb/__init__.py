"""RajeshDB client for Python 3.9+. Standard library only.

SECURITY: an API key is a server-side secret. Keep it in an environment variable or a secret store,
never in source control, and never ship it to browsers or mobile apps.
"""
from __future__ import annotations

import json
import re
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Dict, List, Optional

__all__ = ["Client", "Collection", "RajeshDBError"]
__version__ = "0.1.0"

Doc = Dict[str, Any]


class RajeshDBError(Exception):
    """Raised for every failed request. `status` is 0 for network problems and timeouts."""

    def __init__(self, status: int, code: str, message: str) -> None:
        super().__init__(message)
        self.status = status
        self.code = code
        self.message = message


def _q(s: str) -> str:
    return urllib.parse.quote(s, safe="")


class Client:
    def __init__(self, base_url: str, api_key: str, project: str, env: str = "dev", timeout: float = 30.0) -> None:
        if not api_key or not api_key.startswith("rdb_"):
            raise ValueError('api_key must be a project API key starting with "rdb_"')
        if not re.fullmatch(r"[0-9a-f]{32}", project):
            raise ValueError("project must be the 32-character project id")
        if env not in ("dev", "staging", "prod"):
            raise ValueError("env must be dev, staging or prod")
        self.base_url = base_url.rstrip("/")
        self.project = project
        self.env = env
        self.timeout = timeout
        self._api_key = api_key

    def __repr__(self) -> str:  # never leak the key into logs or tracebacks
        return f"Client(base_url={self.base_url!r}, project={self.project!r}, env={self.env!r}, api_key='[hidden]')"

    def _env_path(self) -> str:
        return f"/v1/projects/{_q(self.project)}/envs/{_q(self.env)}"

    def _request(self, method: str, path: str, body: Any = None, allow_404: bool = False) -> Any:
        data = None if body is None else json.dumps(body).encode("utf-8")
        headers = {"Authorization": f"Bearer {self._api_key}"}
        if data is not None:
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(self.base_url + path, data=data, method=method, headers=headers)
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                text = resp.read().decode("utf-8")
                return json.loads(text) if text else None
        except urllib.error.HTTPError as e:
            text = e.read().decode("utf-8", "replace")
            try:
                payload = json.loads(text) if text else None
            except ValueError:
                payload = None
            if e.code == 404 and allow_404:
                return None
            err = (payload or {}).get("error", {}) if isinstance(payload, dict) else {}
            raise RajeshDBError(e.code, err.get("code", "http_error"), err.get("message", f"HTTP {e.code}")) from None
        except urllib.error.URLError as e:
            reason = getattr(e, "reason", e)
            if isinstance(reason, TimeoutError) or "timed out" in str(reason):
                raise RajeshDBError(0, "timeout", f"request timed out after {self.timeout} s") from None
            raise RajeshDBError(0, "network", "cannot reach the server") from None
        except TimeoutError:
            raise RajeshDBError(0, "timeout", f"request timed out after {self.timeout} s") from None

    def list_collections(self) -> List[str]:
        return self._request("GET", f"{self._env_path()}/collections")["collections"]

    def create_collection(self, name: str, schema: Optional[Doc] = None) -> None:
        """Needs an admin key."""
        self._request("POST", f"{self._env_path()}/collections", {"name": name, **({"schema": schema} if schema else {})})

    def collection(self, name: str) -> "Collection":
        return Collection(self, name)


class Collection:
    def __init__(self, client: Client, name: str) -> None:
        self._c = client
        self.name = name

    def _base(self) -> str:
        return f"{self._c._env_path()}/collections/{_q(self.name)}"

    def insert(self, doc: Doc) -> str:
        """Returns the _id (generated when the document has none)."""
        return self._c._request("POST", f"{self._base()}/documents", doc)["_id"]

    def get(self, doc_id: str) -> Optional[Doc]:
        return self._c._request("GET", f"{self._base()}/documents/{_q(doc_id)}", allow_404=True)

    def replace(self, doc_id: str, doc: Doc) -> None:
        self._c._request("PUT", f"{self._base()}/documents/{_q(doc_id)}", doc)

    def delete(self, doc_id: str) -> bool:
        """True if a document was deleted, False if it did not exist."""
        return self._c._request("DELETE", f"{self._base()}/documents/{_q(doc_id)}", allow_404=True) is not None

    def find(self, filter: Optional[Doc] = None, sort: Any = None, skip: Optional[int] = None, limit: Optional[int] = None) -> Doc:
        body: Doc = {}
        if filter is not None:
            body["filter"] = filter
        if sort is not None:
            body["sort"] = sort
        if skip is not None:
            body["skip"] = skip
        if limit is not None:
            body["limit"] = limit
        return self._c._request("POST", f"{self._base()}/query", body)

    def find_one(self, filter: Doc) -> Optional[Doc]:
        docs = self.find(filter=filter, limit=1)["documents"]
        return docs[0] if docs else None

    def update(self, filter: Doc, set: Doc, confirm_all: bool = False) -> int:
        """Shallow-merges `set` into every matching document. An empty filter needs confirm_all=True."""
        body: Doc = {"filter": filter, "set": set}
        if confirm_all:
            body["confirm_all"] = True
        return self._c._request("POST", f"{self._base()}/update", body)["modified"]

    def delete_where(self, filter: Doc, confirm_all: bool = False) -> int:
        body: Doc = {"filter": filter}
        if confirm_all:
            body["confirm_all"] = True
        return self._c._request("POST", f"{self._base()}/delete", body)["deleted"]

    def create_index(self, field: str) -> None:
        """Needs an admin key."""
        self._c._request("POST", f"{self._base()}/indexes", {"field": field})

    def indexes(self) -> List[str]:
        return self._c._request("GET", f"{self._base()}/indexes")["indexes"]
