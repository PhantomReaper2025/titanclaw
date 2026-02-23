
let gatewayStatusInterval = null;

function startGatewayStatusPolling() {
  fetchGatewayStatus();
  gatewayStatusInterval = setInterval(fetchGatewayStatus, 30000);
}

function fetchGatewayStatus() {
  apiFetch('/api/gateway/status').then((data) => {
    const popover = document.getElementById('gateway-popover');
    popover.innerHTML = '<div class="gw-stat"><span>SSE clients</span><span>' + (data.sse_clients || 0) + '</span></div>'
      + '<div class="gw-stat"><span>Log clients</span><span>' + (data.log_clients || 0) + '</span></div>'
      + '<div class="gw-stat"><span>Uptime</span><span>' + formatDuration(data.uptime_secs) + '</span></div>';
  }).catch(() => {});
}

// Show/hide popover on hover
document.getElementById('gateway-status-trigger').addEventListener('mouseenter', () => {
  document.getElementById('gateway-popover').classList.add('visible');
});
document.getElementById('gateway-status-trigger').addEventListener('mouseleave', () => {
  document.getElementById('gateway-popover').classList.remove('visible');
});
