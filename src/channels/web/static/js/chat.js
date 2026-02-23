
function sendMessage() {
  const input = document.getElementById('chat-input');
  const sendBtn = document.getElementById('send-btn');
  const content = input.value.trim();
  if (!content) return;

  clearPendingAssistantPlaceholder();
  addMessage('user', content);
  createPendingAssistantPlaceholder();
  input.value = '';
  autoResizeTextarea(input);
  setStatus('Sending...', true);

  sendBtn.disabled = true;
  input.disabled = true;

  apiFetch('/api/chat/send', {
    method: 'POST',
    body: { content, thread_id: currentThreadId || undefined },
  }).catch((err) => {
    clearPendingAssistantPlaceholder();
    addMessage('system', 'Failed to send: ' + err.message);
    setStatus('');
    enableChatInput();
  });
}

function enableChatInput() {
  const input = document.getElementById('chat-input');
  const sendBtn = document.getElementById('send-btn');
  sendBtn.disabled = false;
  input.disabled = false;
  input.focus();
}

function sendApprovalAction(requestId, action) {
  apiFetch('/api/chat/approval', {
    method: 'POST',
    body: { request_id: requestId, action: action, thread_id: currentThreadId },
  }).catch((err) => {
    addMessage('system', 'Failed to send approval: ' + err.message);
  });

  // Disable buttons and show confirmation on the card
  const card = document.querySelector('.approval-card[data-request-id="' + requestId + '"]');
  if (card) {
    const buttons = card.querySelectorAll('.approval-actions button');
    buttons.forEach((btn) => {
      btn.disabled = true;
    });
    const actions = card.querySelector('.approval-actions');
    const label = document.createElement('span');
    label.className = 'approval-resolved';
    const labelText = action === 'approve' ? 'Approved' : action === 'always' ? 'Always approved' : 'Denied';
    label.textContent = labelText;
    actions.appendChild(label);
  }
}

function renderMarkdown(text) {
  if (typeof marked !== 'undefined') {
    let html = marked.parse(text);
    // Sanitize HTML output to prevent XSS from tool output or LLM responses.
    html = sanitizeRenderedHtml(html);
    return html;
  }
  return escapeHtml(text);
}

const SANITIZER_ALLOWED_TAGS = new Set([
  'a', 'p', 'br', 'hr', 'blockquote',
  'strong', 'b', 'em', 'i', 'u', 's', 'del',
  'code', 'pre', 'kbd', 'samp',
  'ul', 'ol', 'li',
  'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
  'table', 'thead', 'tbody', 'tr', 'th', 'td',
  'img', 'span', 'div',
  'input',
]);

const SANITIZER_GLOBAL_ATTRS = new Set(['class', 'title']);

const SANITIZER_TAG_ATTRS = {
  a: new Set(['href', 'title', 'target', 'rel']),
  img: new Set(['src', 'alt', 'title']),
  th: new Set(['colspan', 'rowspan', 'align']),
  td: new Set(['colspan', 'rowspan', 'align']),
  input: new Set(['type', 'checked', 'disabled']),
};

const SANITIZER_DROP_ENTIRE_TAGS = new Set([
  'script', 'style', 'iframe', 'object', 'embed', 'form', 'link', 'base', 'meta',
  'svg', 'math',
]);

function unwrapElement(el) {
  const parent = el.parentNode;
  if (!parent) return;
  while (el.firstChild) {
    parent.insertBefore(el.firstChild, el);
  }
  parent.removeChild(el);
}

function isAllowedUrl(value) {
  const raw = String(value || '').trim();
  if (!raw) return false;
  if (raw.startsWith('#')) return true;
  if (raw.startsWith('/')) return true;
  const lowered = raw.toLowerCase();
  if (lowered.startsWith('javascript:') || lowered.startsWith('data:') || lowered.startsWith('vbscript:')) {
    return false;
  }
  try {
    const parsed = new URL(raw, window.location.origin);
    return ['http:', 'https:', 'mailto:', 'tel:'].includes(parsed.protocol);
  } catch (_) {
    return false;
  }
}

// Sanitize rendered markdown by parsing it into a DOM tree and applying an allowlist.
// This avoids regex-based HTML filtering, which is fragile and bypassable.
function sanitizeRenderedHtml(html) {
  if (typeof DOMParser === 'undefined') return '';

  const doc = new DOMParser().parseFromString('<!doctype html><body>' + html, 'text/html');
  const elements = Array.from(doc.body.querySelectorAll('*'));

  for (const el of elements) {
    const tag = el.tagName.toLowerCase();

    if (SANITIZER_DROP_ENTIRE_TAGS.has(tag)) {
      el.remove();
      continue;
    }

    if (!SANITIZER_ALLOWED_TAGS.has(tag)) {
      unwrapElement(el);
      continue;
    }

    const allowedForTag = SANITIZER_TAG_ATTRS[tag] || new Set();
    for (const attr of Array.from(el.attributes)) {
      const name = attr.name.toLowerCase();
      if (name.startsWith('on')) {
        el.removeAttribute(attr.name);
        continue;
      }
      if (!SANITIZER_GLOBAL_ATTRS.has(name) && !allowedForTag.has(name)) {
        el.removeAttribute(attr.name);
      }
    }

    if (tag === 'a') {
      const href = el.getAttribute('href');
      if (!href || !isAllowedUrl(href)) {
        el.removeAttribute('href');
      }
      if (el.getAttribute('target') === '_blank') {
        el.setAttribute('rel', 'noopener noreferrer');
      }
    }

    if (tag === 'img') {
      const src = el.getAttribute('src');
      if (!src || !isAllowedUrl(src)) {
        el.remove();
        continue;
      }
      el.removeAttribute('srcset');
    }

    if (tag === 'input') {
      const type = (el.getAttribute('type') || '').toLowerCase();
      if (type !== 'checkbox') {
        el.remove();
        continue;
      }
      el.setAttribute('disabled', '');
    }
  }

  return doc.body.innerHTML;
}

function decorateRenderedMarkdown(root) {
  if (!root || !root.querySelectorAll) return;
  root.querySelectorAll('pre').forEach((pre) => {
    pre.classList.add('code-block-wrapper');
    let copyBtn = null;
    for (const child of Array.from(pre.children)) {
      if (child.classList && child.classList.contains('copy-btn')) {
        copyBtn = child;
        break;
      }
    }
    if (!copyBtn) {
      copyBtn = document.createElement('button');
      copyBtn.type = 'button';
      copyBtn.className = 'copy-btn';
      copyBtn.textContent = 'Copy';
      copyBtn.addEventListener('click', () => copyCodeBlock(copyBtn));
      pre.insertBefore(copyBtn, pre.firstChild);
    }
  });
}

function copyCodeBlock(btn) {
  const pre = btn.parentElement;
  const code = pre.querySelector('code');
  const text = code ? code.textContent : pre.textContent;
  navigator.clipboard.writeText(text).then(() => {
    btn.textContent = 'Copied!';
    setTimeout(() => { btn.textContent = 'Copy'; }, 1500);
  });
}

function addMessage(role, content) {
  const container = document.getElementById('chat-messages');
  const div = createMessageElement(role, content);
  container.appendChild(div);
  container.scrollTop = container.scrollHeight;
}

function createPendingAssistantPlaceholder() {
  const container = document.getElementById('chat-messages');
  const div = createMessageElement('assistant', '');
  div.classList.add('is-streaming');
  div.setAttribute('data-pending-response', '1');
  container.appendChild(div);
  container.scrollTop = container.scrollHeight;
  return div;
}

function getActiveAssistantBubble() {
  const container = document.getElementById('chat-messages');
  const active = container.querySelectorAll('.message.assistant.is-streaming, .message.assistant[data-pending-response="1"]');
  if (active.length === 0) return null;
  return active[active.length - 1];
}

function clearPendingAssistantPlaceholder() {
  const active = getActiveAssistantBubble();
  if (!active) return;
  const raw = (active.getAttribute('data-raw') || '').trim();
  if (!raw) {
    active.remove();
  }
}

function appendToLastAssistant(chunk) {
  const container = document.getElementById('chat-messages');
  let last = getActiveAssistantBubble();
  if (!last) {
    last = createPendingAssistantPlaceholder();
  }
  last.classList.add('is-streaming');
  const raw = (last.getAttribute('data-raw') || '') + chunk;
  last.setAttribute('data-raw', raw);
  last.innerHTML = raw ? renderMarkdown(raw) : '';
  decorateRenderedMarkdown(last);
  container.scrollTop = container.scrollHeight;
}

function finalizeAssistantResponse(content) {
  const container = document.getElementById('chat-messages');
  const active = getActiveAssistantBubble();
  if (active) {
    active.classList.remove('is-streaming');
    active.removeAttribute('data-pending-response');
    active.setAttribute('data-raw', content);
    active.innerHTML = renderMarkdown(content);
    decorateRenderedMarkdown(active);
    container.scrollTop = container.scrollHeight;
    return;
  }

  const messages = container.querySelectorAll('.message.assistant');
  if (messages.length === 0) {
    addMessage('assistant', content);
    return;
  }

  const last = messages[messages.length - 1];
  addMessage('assistant', content);
}

function setStatus(text, spinning) {
  const el = document.getElementById('chat-status');
  if (!text) {
    el.innerHTML = '';
    el.classList.remove('has-status');
    return;
  }
  el.classList.add('has-status');
  el.innerHTML = (spinning ? '<div class="spinner"></div>' : '')
    + '<span class="status-text">' + escapeHtml(text) + '</span>';
}

function showApproval(data) {
  const container = document.getElementById('chat-messages');
  const card = document.createElement('div');
  card.className = 'approval-card';
  card.setAttribute('data-request-id', data.request_id);

  const header = document.createElement('div');
  header.className = 'approval-header';
  header.textContent = 'Tool requires approval';
  card.appendChild(header);

  const toolName = document.createElement('div');
  toolName.className = 'approval-tool-name';
  toolName.textContent = data.tool_name;
  card.appendChild(toolName);

  if (data.description) {
    const desc = document.createElement('div');
    desc.className = 'approval-description';
    desc.textContent = data.description;
    card.appendChild(desc);
  }

  if (data.parameters) {
    const paramsToggle = document.createElement('button');
    paramsToggle.className = 'approval-params-toggle';
    paramsToggle.textContent = 'Show parameters';
    const paramsBlock = document.createElement('pre');
    paramsBlock.className = 'approval-params';
    paramsBlock.textContent = data.parameters;
    paramsBlock.style.display = 'none';
    paramsToggle.addEventListener('click', () => {
      const visible = paramsBlock.style.display !== 'none';
      paramsBlock.style.display = visible ? 'none' : 'block';
      paramsToggle.textContent = visible ? 'Show parameters' : 'Hide parameters';
    });
    card.appendChild(paramsToggle);
    card.appendChild(paramsBlock);
  }

  const actions = document.createElement('div');
  actions.className = 'approval-actions';

  const approveBtn = document.createElement('button');
  approveBtn.className = 'approve';
  approveBtn.textContent = 'Approve';
  approveBtn.addEventListener('click', () => sendApprovalAction(data.request_id, 'approve'));

  const alwaysBtn = document.createElement('button');
  alwaysBtn.className = 'always';
  alwaysBtn.textContent = 'Always';
  alwaysBtn.addEventListener('click', () => sendApprovalAction(data.request_id, 'always'));

  const denyBtn = document.createElement('button');
  denyBtn.className = 'deny';
  denyBtn.textContent = 'Deny';
  denyBtn.addEventListener('click', () => sendApprovalAction(data.request_id, 'deny'));

  actions.appendChild(approveBtn);
  actions.appendChild(alwaysBtn);
  actions.appendChild(denyBtn);
  card.appendChild(actions);

  container.appendChild(card);
  container.scrollTop = container.scrollHeight;
}

function showJobCard(data) {
  const container = document.getElementById('chat-messages');
  const card = document.createElement('div');
  card.className = 'job-card';

  const icon = document.createElement('span');
  icon.className = 'job-card-icon';
  icon.textContent = '\u2692';
  card.appendChild(icon);

  const info = document.createElement('div');
  info.className = 'job-card-info';

  const title = document.createElement('div');
  title.className = 'job-card-title';
  title.textContent = data.title || 'Sandbox Job';
  info.appendChild(title);

  const id = document.createElement('div');
  id.className = 'job-card-id';
  id.textContent = (data.job_id || '').substring(0, 8);
  info.appendChild(id);

  card.appendChild(info);

  const viewBtn = document.createElement('button');
  viewBtn.className = 'job-card-view';
  viewBtn.textContent = 'View Job';
  viewBtn.addEventListener('click', () => {
    switchTab('jobs');
    openJobDetail(data.job_id);
  });
  card.appendChild(viewBtn);

  if (data.browse_url) {
    const browseBtn = document.createElement('a');
    browseBtn.className = 'job-card-browse';
    browseBtn.href = data.browse_url;
    browseBtn.target = '_blank';
    browseBtn.textContent = 'Browse';
    card.appendChild(browseBtn);
  }

  container.appendChild(card);
  container.scrollTop = container.scrollHeight;
}
