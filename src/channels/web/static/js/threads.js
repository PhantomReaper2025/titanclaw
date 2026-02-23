
function loadThreads() {
  apiFetch('/api/chat/threads').then((data) => {
    // Pinned assistant thread
    if (data.assistant_thread) {
      assistantThreadId = data.assistant_thread.id;
      const el = document.getElementById('assistant-thread');
      const isActive = currentThreadId === assistantThreadId;
      el.className = 'assistant-item' + (isActive ? ' active' : '');
      const meta = document.getElementById('assistant-meta');
      const count = data.assistant_thread.turn_count || 0;
      meta.textContent = count > 0 ? count + ' turns' : '';
    }

    // Regular threads
    const list = document.getElementById('thread-list');
    list.innerHTML = '';
    const threads = data.threads || [];
    for (const thread of threads) {
      const item = document.createElement('div');
      item.className = 'thread-item' + (thread.id === currentThreadId ? ' active' : '');
      const label = document.createElement('span');
      label.className = 'thread-label';
      label.textContent = thread.title || thread.id.substring(0, 8);
      label.title = thread.title ? thread.title + ' (' + thread.id + ')' : thread.id;
      item.appendChild(label);
      const meta = document.createElement('span');
      meta.className = 'thread-meta';
      meta.textContent = (thread.turn_count || 0) + ' turns';
      const actions = document.createElement('div');
      actions.className = 'thread-actions';
      actions.appendChild(meta);
      const delBtn = document.createElement('button');
      delBtn.className = 'thread-delete-btn';
      delBtn.textContent = 'Delete';
      delBtn.title = 'Delete this chat thread';
      delBtn.addEventListener('click', (ev) => {
        ev.stopPropagation();
        deleteThread(thread.id, thread.title || thread.id.substring(0, 8));
      });
      actions.appendChild(delBtn);
      item.appendChild(actions);
      item.addEventListener('click', () => switchThread(thread.id));
      list.appendChild(item);
    }

    // Default to assistant thread on first load if no thread selected
    if (!currentThreadId && assistantThreadId) {
      switchToAssistant();
    }
  }).catch(() => {});
}

function switchToAssistant() {
  if (!assistantThreadId) return;
  currentThreadId = assistantThreadId;
  hasMore = false;
  oldestTimestamp = null;
  loadHistory();
  loadThreads();
}

function switchThread(threadId) {
  currentThreadId = threadId;
  hasMore = false;
  oldestTimestamp = null;
  loadHistory();
  loadThreads();
}

function createNewThread() {
  apiFetch('/api/chat/thread/new', { method: 'POST' }).then((data) => {
    currentThreadId = data.id || null;
    document.getElementById('chat-messages').innerHTML = '';
    setStatus('');
    loadThreads();
  }).catch((err) => {
    showToast('Failed to create thread: ' + err.message, 'error');
  });
}

function deleteThread(threadId, title) {
  if (!confirm('Delete chat "' + title + '"? This cannot be undone.')) return;
  apiFetch('/api/chat/thread/' + encodeURIComponent(threadId), { method: 'DELETE' })
    .then((data) => {
      if (currentThreadId === threadId) {
        currentThreadId = data.active_thread || assistantThreadId || null;
        loadHistory();
      }
      loadThreads();
      showToast('Thread deleted', 'success');
    })
    .catch((err) => showToast('Delete failed: ' + err.message, 'error'));
}

function clearAllChats() {
  if (!confirm('Delete ALL chat threads? This cannot be undone.')) return;
  apiFetch('/api/chat/threads', { method: 'DELETE' })
    .then((data) => {
      currentThreadId = data.assistant_thread || null;
      document.getElementById('chat-messages').innerHTML = '';
      hasMore = false;
      oldestTimestamp = null;
      loadThreads();
      showToast('All chats cleared', 'success');
    })
    .catch((err) => showToast('Clear failed: ' + err.message, 'error'));
}

function toggleThreadSidebar() {
  const sidebar = document.getElementById('thread-sidebar');
  if (window.innerWidth <= 768) {
    sidebar.classList.toggle('expanded-mobile');
    return;
  }
  sidebar.classList.toggle('collapsed');
  const btn = document.getElementById('thread-toggle-btn');
  btn.innerHTML = sidebar.classList.contains('collapsed') ? '&raquo;' : '&laquo;';
}

// Chat input auto-resize and keyboard handling
const chatInput = document.getElementById('chat-input');
chatInput.addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    sendMessage();
  }
});
chatInput.addEventListener('input', () => autoResizeTextarea(chatInput));

// Infinite scroll: load older messages when scrolled near the top
document.getElementById('chat-messages').addEventListener('scroll', function () {
  if (this.scrollTop < 100 && hasMore && !loadingOlder) {
    loadingOlder = true;
    // Show spinner at top
    const spinner = document.createElement('div');
    spinner.id = 'scroll-load-spinner';
    spinner.className = 'scroll-load-spinner';
    spinner.innerHTML = '<div class="spinner"></div> Loading older messages...';
    this.insertBefore(spinner, this.firstChild);
    loadHistory(oldestTimestamp);
  }
});

function autoResizeTextarea(el) {
  el.style.height = 'auto';
  el.style.height = Math.min(el.scrollHeight, 120) + 'px';
}
