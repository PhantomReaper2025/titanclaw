// IronClaw Web Gateway - Client

let token = '';
let eventSource = null;
let logEventSource = null;
let chatReconnectTimer = null;
let logReconnectTimer = null;
let chatReconnectAttempts = 0;
let logReconnectAttempts = 0;
let chatWatchdogTimer = null;
let logWatchdogTimer = null;
let chatLastActivityAt = 0;
let logLastActivityAt = 0;
let currentTab = 'chat';
let currentThreadId = null;
let assistantThreadId = null;
let hasMore = false;
let oldestTimestamp = null;
let loadingOlder = false;
let jobEvents = new Map(); // job_id -> Array of events
let jobListRefreshTimer = null;
const JOB_EVENTS_CAP = 500;
const MEMORY_SEARCH_QUERY_MAX_LENGTH = 100;

function reconnectDelay(attempt) {
  const base = Math.min(6000, 250 * Math.pow(1.8, Math.min(attempt, 7)));
  const jitter = Math.floor(Math.random() * 150);
  return base + jitter;
}

function scheduleChatReconnect() {
  if (chatReconnectTimer || !token) return;
  const delay = reconnectDelay(chatReconnectAttempts);
  chatReconnectTimer = setTimeout(() => {
    chatReconnectTimer = null;
    chatReconnectAttempts += 1;
    connectSSE();
  }, delay);
}

function touchChatActivity() {
  chatLastActivityAt = Date.now();
}

function touchLogActivity() {
  logLastActivityAt = Date.now();
}

function forceChatReconnectNow() {
  if (!token) return;
  if (eventSource) eventSource.close();
  if (chatReconnectTimer) {
    clearTimeout(chatReconnectTimer);
    chatReconnectTimer = null;
  }
  connectSSE();
}

function forceLogReconnectNow() {
  if (!token) return;
  if (logEventSource) logEventSource.close();
  if (logReconnectTimer) {
    clearTimeout(logReconnectTimer);
    logReconnectTimer = null;
  }
  connectLogSSE();
}

function startSseWatchdogs() {
  if (chatWatchdogTimer) clearInterval(chatWatchdogTimer);
  chatWatchdogTimer = setInterval(() => {
    if (!token || !eventSource) return;
    const stalled = Date.now() - chatLastActivityAt > 15000;
    const closed = eventSource.readyState === EventSource.CLOSED;
    if (stalled || closed) scheduleChatReconnect();
  }, 4000);

  if (logWatchdogTimer) clearInterval(logWatchdogTimer);
  logWatchdogTimer = setInterval(() => {
    if (!token || !logEventSource) return;
    const stalled = Date.now() - logLastActivityAt > 15000;
    const closed = logEventSource.readyState === EventSource.CLOSED;
    if (stalled || closed) scheduleLogReconnect();
  }, 4000);
}

function scheduleLogReconnect() {
  if (logReconnectTimer || !token) return;
  const delay = reconnectDelay(logReconnectAttempts);
  logReconnectTimer = setTimeout(() => {
    logReconnectTimer = null;
    logReconnectAttempts += 1;
    connectLogSSE();
  }, delay);
}
