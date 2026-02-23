
function authenticate() {
  token = document.getElementById('token-input').value.trim();
  if (!token) {
    document.getElementById('auth-error').textContent = 'Token required';
    return;
  }

  // Test the token against the health-ish endpoint (chat/threads requires auth)
  apiFetch('/api/chat/threads')
    .then(() => {
      sessionStorage.setItem('ironclaw_token', token);
      document.getElementById('auth-screen').style.display = 'none';
      document.getElementById('app').style.display = 'flex';
      // Strip token from URL so it's not visible in the address bar
      const cleaned = new URL(window.location);
      cleaned.searchParams.delete('token');
      window.history.replaceState({}, '', cleaned.pathname + cleaned.search);
      connectSSE();
      connectLogSSE();
      startSseWatchdogs();
      startGatewayStatusPolling();
      loadThreads();
      loadMemoryTree();
      loadJobs();
    })
    .catch(() => {
      sessionStorage.removeItem('ironclaw_token');
      document.getElementById('auth-screen').style.display = '';
      document.getElementById('app').style.display = 'none';
      document.getElementById('auth-error').textContent = 'Invalid token';
    });
}

function runAutoAuth() {
  const params = new URLSearchParams(window.location.search);
  const urlToken = params.get('token');
  if (urlToken) {
    document.getElementById('token-input').value = urlToken;
    authenticate();
    return;
  }
  const saved = sessionStorage.getItem('ironclaw_token');
  if (saved) {
    document.getElementById('token-input').value = saved;
    // Hide auth screen immediately to prevent flash, authenticate() will
    // restore it if the token turns out to be invalid.
    document.getElementById('auth-screen').style.display = 'none';
    document.getElementById('app').style.display = 'flex';
    authenticate();
  }
}

document.getElementById('token-input').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') authenticate();
});

// Defer auto-auth until the page finishes loading so split modules
// (api_helper.js, sse.js, logs.js, etc.) are already defined.
window.addEventListener('load', runAutoAuth);

window.addEventListener('online', () => {
  forceChatReconnectNow();
  forceLogReconnectNow();
});

document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible') {
    forceChatReconnectNow();
    forceLogReconnectNow();
  }
});
