
async function apiFetch(path, options) {
  const opts = options || {};
  opts.headers = opts.headers || {};
  opts.headers['Authorization'] = 'Bearer ' + token;

  const body = opts.body;
  const isJsonObjectBody = body != null
    && typeof body === 'object'
    && !(body instanceof FormData)
    && !(body instanceof Blob)
    && !(body instanceof URLSearchParams);
  if (isJsonObjectBody) {
    opts.headers['Content-Type'] = 'application/json';
    opts.body = JSON.stringify(body);
  }

  const res = await fetch(path, opts);
  const contentType = (res.headers.get('content-type') || '').toLowerCase();

  if (!res.ok) {
    let detail = '';
    try {
      if (contentType.includes('application/json')) {
        const data = await res.json();
        detail = data && (data.message || data.error || data.detail || '');
        if (!detail) detail = JSON.stringify(data);
      } else {
        detail = (await res.text()).trim();
      }
    } catch (_) {
      detail = '';
    }
    const suffix = detail ? ': ' + detail : ' ' + res.statusText;
    throw new Error(String(res.status) + suffix);
  }

  if (res.status === 204) return null;
  if (contentType.includes('application/json')) return res.json();
  return res.text();
}
