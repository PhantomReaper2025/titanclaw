
function installExtension() {
  const name = document.getElementById('ext-install-name').value.trim();
  if (!name) {
    showToast('Extension name is required', 'error');
    return;
  }
  const url = document.getElementById('ext-install-url').value.trim();
  const kind = document.getElementById('ext-install-kind').value;

  apiFetch('/api/extensions/install', {
    method: 'POST',
    body: { name, url: url || undefined, kind },
  }).then((res) => {
    if (res.success) {
      showToast('Installed ' + name, 'success');
      document.getElementById('ext-install-name').value = '';
      document.getElementById('ext-install-url').value = '';
      loadExtensions();
    } else {
      showToast('Install failed: ' + (res.message || 'unknown error'), 'error');
    }
  }).catch((err) => {
    showToast('Install failed: ' + err.message, 'error');
  });
}
