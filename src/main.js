// ═══════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Main Application
// Routage SPA + State global + IPC Tauri + i18n
// ═══════════════════════════════════════════════════════
//
// Toutes les chaînes de texte passent par t('clé') ou
// tPlural('clé', count) définis dans i18n.js.
// Le français reste en dur dans le HTML comme fallback.
//

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ─── State global de l'application ───
const AppState = {
    token: null,
    user: null,
    currentScreen: 'login',
    selectedEvent: null,
    selectedCheckpoint: null,
    sessionId: null,
    watchFolder: null,
    parallelUploads: 1,
    includeExisting: true,
    isPaused: false,
};

// ─── Routeur SPA ───

function navigateTo(screenId) {
    // Masquer tous les écrans
    document.querySelectorAll('.screen').forEach(el => {
        el.classList.remove('active');
    });
    // Afficher l'écran demandé
    const screen = document.getElementById(`screen-${screenId}`);
    if (screen) {
        screen.classList.add('active');
        AppState.currentScreen = screenId;
    }
}

// ─── Initialisation au lancement ───

document.addEventListener('DOMContentLoaded', async () => {
    // 1. Charger les traductions AVANT tout le reste
    await initI18n();

    // 2. Vérifier si un token est stocké (auto-login)
    try {
        const stored = await invoke('get_stored_auth');
        if (stored) {
            AppState.token = stored.token;
            AppState.user = stored.user;
            updateUserDisplay();
            navigateTo('events');
            loadEvents();
        } else {
            navigateTo('login');
        }
    } catch (e) {
        console.error('Auto-login error:', e);
        navigateTo('login');
    }

    // 3. Initialiser les écrans
    initLogin();
    initEvents();
    initConfig();
    initDashboard();

    // 4. Écouter les événements du backend Rust
    initRustEventListeners();
});

// ═══════════════════════════════════════════════════════
// ÉCRAN LOGIN
// ═══════════════════════════════════════════════════════

function initLogin() {
    const form = document.getElementById('login-form');
    const btn = document.getElementById('login-btn');
    const errorDiv = document.getElementById('login-error');

    form.addEventListener('submit', async (e) => {
        e.preventDefault();
        const email = document.getElementById('login-email').value.trim();
        const password = document.getElementById('login-password').value;
        const remember = document.getElementById('login-remember').checked;

        if (!email || !password) return;

        // Afficher le loader
        btn.disabled = true;
        btn.querySelector('.btn-text').style.display = 'none';
        btn.querySelector('.btn-loader').style.display = 'inline-flex';
        errorDiv.style.display = 'none';

        try {
            const result = await invoke('login', { email, password, remember });
            AppState.token = result.token;
            AppState.user = result.user;
            updateUserDisplay();

            // Nettoyer le formulaire
            form.reset();
            document.getElementById('login-remember').checked = true;

            navigateTo('events');
            loadEvents();
        } catch (error) {
            errorDiv.textContent = error;
            errorDiv.style.display = 'block';
        } finally {
            btn.disabled = false;
            btn.querySelector('.btn-text').style.display = 'inline';
            btn.querySelector('.btn-loader').style.display = 'none';
        }
    });
}

// ═══════════════════════════════════════════════════════
// ÉCRAN ÉVÉNEMENTS
// ═══════════════════════════════════════════════════════

function initEvents() {
    document.getElementById('logout-btn').addEventListener('click', async () => {
        try {
            await invoke('logout');
        } catch (e) {
            console.error('Logout error:', e);
        }
        AppState.token = null;
        AppState.user = null;
        navigateTo('login');
    });
}

async function loadEvents() {
    const listDiv = document.getElementById('events-list');
    const loadingDiv = document.getElementById('events-loading');
    const errorDiv = document.getElementById('events-error');

    listDiv.innerHTML = '';
    loadingDiv.style.display = 'block';
    errorDiv.style.display = 'none';

    try {
        const events = await invoke('fetch_events', { token: AppState.token });

        loadingDiv.style.display = 'none';

        if (events.length === 0) {
            listDiv.innerHTML = `<div class="log-empty">${escapeHtml(t('events.empty'))}</div>`;
            return;
        }

        events.forEach(event => {
            const sportIcons = {
                'running': '🏃',
                'cycling': '🚴',
                'triathlon': '🏊',
                'trail': '🏔️',
                'swimming': '🏊',
                'other': '📷',
            };
            const icon = sportIcons[event.sport_type] || '📷';

            const photoLabel = event.photo_count === 0
                ? t('events.no_photos')
                : tPlural('events.photos', event.photo_count);

            const cpLabel = tPlural('events.checkpoints', event.checkpoints.length);

            const liveHtml = event.is_live
                ? '<span class="event-live">● LIVE</span>'
                : '';

            const card = document.createElement('div');
            card.className = 'event-card';
            card.innerHTML = `
                <div class="event-card-header">
                    <span class="event-icon">${icon}</span>
                    <span class="event-name">${escapeHtml(event.name)}</span>
                    ${liveHtml}
                </div>
                <div class="event-meta">
                    <span>${event.event_date}</span>
                    <span>${escapeHtml(photoLabel)}</span>
                    <span>${escapeHtml(cpLabel)}</span>
                </div>
            `;
            card.addEventListener('click', () => selectEvent(event));
            listDiv.appendChild(card);
        });

    } catch (error) {
        loadingDiv.style.display = 'none';
        if (error === 'SESSION_EXPIRED') {
            AppState.token = null;
            navigateTo('login');
            return;
        }
        errorDiv.textContent = error;
        errorDiv.style.display = 'block';
    }
}

function selectEvent(event) {
    AppState.selectedEvent = event;

    // Remplir l'écran de configuration
    document.getElementById('config-event-name').textContent = event.name;
    document.getElementById('config-user-name').textContent = AppState.user.name;

    // Remplir le select des checkpoints
    const select = document.getElementById('config-checkpoint');
    select.innerHTML = `<option value="">${escapeHtml(t('config.checkpoint_none'))}</option>`;
    event.checkpoints.forEach(cp => {
        const option = document.createElement('option');
        option.value = cp.id;
        option.textContent = cp.name;
        select.appendChild(option);
    });

    // Reset du dossier
    document.getElementById('config-folder').value = '';
    AppState.watchFolder = null;

    navigateTo('config');
}

// ═══════════════════════════════════════════════════════
// ÉCRAN CONFIGURATION
// ═══════════════════════════════════════════════════════

function initConfig() {
    // Bouton retour
    document.getElementById('config-back-btn').addEventListener('click', () => {
        navigateTo('events');
    });

    // Bouton parcourir (dialogue natif)
    document.getElementById('config-browse-btn').addEventListener('click', async () => {
        try {
            const { open } = window.__TAURI__.dialog;
            const selected = await open({
                directory: true,
                multiple: false,
                title: t('config.folder_dialog'),
            });
            if (selected) {
                document.getElementById('config-folder').value = selected;
                AppState.watchFolder = selected;
            }
        } catch (e) {
            console.error('Folder dialog error:', e);
        }
    });

    // Sélecteur uploads parallèles
    document.querySelectorAll('.parallel-btn').forEach(btn => {
        btn.addEventListener('click', () => {
            document.querySelectorAll('.parallel-btn').forEach(b => b.classList.remove('active'));
            btn.classList.add('active');
            AppState.parallelUploads = parseInt(btn.dataset.value);
        });
    });

    // Radio fichiers existants
    document.querySelectorAll('input[name="existing-files"]').forEach(radio => {
        radio.addEventListener('change', () => {
            AppState.includeExisting = radio.value === 'include';
        });
    });

    // Bouton démarrer
    document.getElementById('config-start-btn').addEventListener('click', () => {
        if (!AppState.watchFolder) {
            alert(t('config.folder_required'));
            return;
        }
        startSession();
    });
}

async function startSession() {
    const event = AppState.selectedEvent;
    const cpSelect = document.getElementById('config-checkpoint');
    const checkpointId = cpSelect.value ? parseInt(cpSelect.value) : null;
    const checkpointName = cpSelect.value ? cpSelect.options[cpSelect.selectedIndex].text : null;

    AppState.selectedCheckpoint = checkpointId ? { id: checkpointId, name: checkpointName } : null;

    // Remplir le dashboard
    document.getElementById('dash-event-name').textContent = event.name;
    document.getElementById('dash-checkpoint-name').textContent = checkpointName || '';
    document.getElementById('dash-checkpoint-name').style.display = checkpointName ? 'inline-block' : 'none';
    document.getElementById('dash-folder').textContent = AppState.watchFolder;
    document.getElementById('dash-folder').title = AppState.watchFolder;

    // Reset des stats
    document.getElementById('stat-sent').textContent = '0';
    document.getElementById('stat-pending').textContent = '0';
    document.getElementById('stat-failed').textContent = '0';
    document.getElementById('stat-speed').textContent = '—';
    document.getElementById('progress-fill').style.width = '0%';
    document.getElementById('progress-text').textContent = '0 / 0';
    document.getElementById('upload-log').innerHTML = `<div class="log-empty">${escapeHtml(t('dashboard.log_empty'))}</div>`;

    // Reset de l'état pause
    AppState.isPaused = false;
    const pauseBtn = document.getElementById('dash-pause-btn');
    pauseBtn.innerHTML = `
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/>
        </svg>
        <span>${escapeHtml(t('dashboard.pause'))}</span>`;
    document.getElementById('dash-status').className = 'status-dot status-active';

    navigateTo('dashboard');

    // ── Appeler le backend Rust pour démarrer la surveillance ──
    try {
        const result = await invoke('start_session', {
            token: AppState.token,
            eventId: event.id,
            eventName: event.name,
            checkpointId: checkpointId,
            checkpointName: checkpointName,
            folder: AppState.watchFolder,
            includeExisting: AppState.includeExisting,
            parallel: AppState.parallelUploads,
        });

        AppState.sessionId = result.session_id;
        addLogEntry(timeNow(), '—', 'success', result.message);

    } catch (error) {
        addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error }));
    }
}

// ═══════════════════════════════════════════════════════
// ÉCRAN DASHBOARD
// ═══════════════════════════════════════════════════════

function initDashboard() {
    // ── Bouton Pause / Reprendre ──
    document.getElementById('dash-pause-btn').addEventListener('click', async () => {
        const btn = document.getElementById('dash-pause-btn');
        const dot = document.getElementById('dash-status');

        try {
            if (!AppState.isPaused) {
                // Mettre en pause
                await invoke('pause_session');
                AppState.isPaused = true;
                btn.innerHTML = `
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <polygon points="5 3 19 12 5 21 5 3"/>
                    </svg>
                    <span>${escapeHtml(t('dashboard.resume'))}</span>`;
                dot.className = 'status-dot status-paused';
                addLogEntry(timeNow(), '—', 'retry', t('dashboard.msg_paused'));
            } else {
                // Reprendre
                await invoke('resume_session');
                AppState.isPaused = false;
                btn.innerHTML = `
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/>
                    </svg>
                    <span>${escapeHtml(t('dashboard.pause'))}</span>`;
                dot.className = 'status-dot status-active';
                addLogEntry(timeNow(), '—', 'success', t('dashboard.msg_resumed'));
            }
        } catch (error) {
            console.error('Pause/resume error:', error);
        }
    });

    // ── Bouton Arrêter ──
    document.getElementById('dash-stop-btn').addEventListener('click', async () => {
        if (confirm(t('dashboard.confirm_stop'))) {
            try {
                await invoke('stop_session');
                addLogEntry(timeNow(), '—', 'retry', t('dashboard.msg_stopped'));
            } catch (e) {
                console.error('Stop error:', e);
            }
            AppState.sessionId = null;
            AppState.isPaused = false;
            navigateTo('events');
            loadEvents();
        }
    });

    // ── Bouton Relancer les échecs ──
    document.getElementById('dash-retry-btn').addEventListener('click', async () => {
        if (AppState.sessionId) {
            try {
                const count = await invoke('retry_failed', { sessionId: AppState.sessionId });
                addLogEntry(timeNow(), '—', 'retry', t('dashboard.msg_retry_queued', { count }));
            } catch (e) {
                console.error('Retry error:', e);
            }
        }
    });
}

// ═══════════════════════════════════════════════════════
// ÉVÉNEMENTS RUST → JS
// ═══════════════════════════════════════════════════════
//
// Le backend Rust émet des événements vers le frontend via emit().
// Ici on écoute chacun de ces événements et on met à jour le
// dashboard en temps réel : compteurs, barre de progression, journal.
//

async function initRustEventListeners() {

    // ── Nouveau fichier détecté par le watcher ──
    await listen('file_detected', (event) => {
        const { filename, size } = event.payload;
        const sizeMo = (size / 1048576).toFixed(1);
        addLogEntry(timeNow(), filename, 'uploading', t('watcher.detected', { size: sizeMo }));
    });

    // ── Fichier ignoré (trop gros, etc.) ──
    await listen('file_skipped', (event) => {
        const { filename, reason } = event.payload;
        addLogEntry(timeNow(), filename, 'retry', reason);
    });

    // ── Upload démarré ──
    await listen('upload_started', (event) => {
        const { filename, attempt } = event.payload;
        const msg = attempt > 1
            ? t('upload.attempt', { attempt })
            : t('upload.in_progress');
        addLogEntry(timeNow(), filename, 'uploading', msg);
    });

    // ── Upload réussi ──
    await listen('upload_success', (event) => {
        const { filename, duration_ms, speed_kbps } = event.payload;
        const durationSec = (duration_ms / 1000).toFixed(1);
        addLogEntry(timeNow(), filename, 'success', t('upload.success', { duration: durationSec, speed: speed_kbps }));
    });

    // ── Upload échoué ──
    await listen('upload_failed', (event) => {
        const { filename, error, attempt, max_attempts } = event.payload;
        addLogEntry(timeNow(), filename, 'failed', t('upload.failed', { attempt, max: max_attempts, error }));
    });

    // ── Retry programmé ──
    await listen('upload_retry', (event) => {
        const { filename, attempt, delay_seconds } = event.payload;
        addLogEntry(timeNow(), filename, 'retry', t('upload.retry_scheduled', { delay: delay_seconds, attempt }));
    });

    // ── Stats mises à jour (après chaque upload) ──
    await listen('stats_updated', (event) => {
        const { sent, pending, failed, speed_kbps } = event.payload;
        updateStats(sent, pending, failed, speed_kbps);
    });

    // ── Tous les fichiers traités ──
    await listen('all_complete', (event) => {
        const { total_sent, total_failed } = event.payload;
        const msg = total_failed > 0
            ? t('complete.with_errors', { sent: total_sent, failed: total_failed })
            : t('complete.success', { count: total_sent });
        addLogEntry(timeNow(), '—', 'success', msg);
    });

    // ── Changement d'état réseau ──
    await listen('connection_status', (event) => {
        const { online } = event.payload;
        const dot = document.getElementById('dash-status');
        if (online) {
            dot.className = AppState.isPaused ? 'status-dot status-paused' : 'status-dot status-active';
            addLogEntry(timeNow(), '—', 'success', t('connection.restored'));
        } else {
            dot.className = 'status-dot status-offline';
            addLogEntry(timeNow(), '—', 'failed', t('connection.lost'));
        }
    });

    // ── Session expirée (token Sanctum invalide) ──
    await listen('session_expired', () => {
        addLogEntry(timeNow(), '—', 'failed', t('connection.expired'));
        AppState.token = null;
        AppState.sessionId = null;
        // Petit délai pour que l'utilisateur voie le message
        setTimeout(() => {
            navigateTo('login');
        }, 2000);
    });

    // ── Scan des fichiers existants ──
    await listen('scan_started', (event) => {
        const { total } = event.payload;
        addLogEntry(timeNow(), '—', 'uploading', t('watcher.scan_started', { count: total }));
    });

    await listen('scan_complete', () => {
        addLogEntry(timeNow(), '—', 'success', t('watcher.scan_complete'));
    });
}

// ═══════════════════════════════════════════════════════
// UTILITAIRES
// ═══════════════════════════════════════════════════════

function updateUserDisplay() {
    if (AppState.user) {
        document.getElementById('user-name').textContent = AppState.user.name;
        document.getElementById('config-user-name').textContent = AppState.user.name;
    }
}

function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

/// Retourne l'heure actuelle au format HH:MM:SS pour le journal
/// Utilise la locale détectée par i18n pour le format d'heure
function timeNow() {
    const now = new Date();
    const locale = typeof getCurrentLocale === 'function' ? getCurrentLocale() : 'fr';
    return now.toLocaleTimeString(locale, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

// ─── Journal du dashboard ───

function addLogEntry(time, filename, status, message) {
    const log = document.getElementById('upload-log');
    if (!log) return;

    const empty = log.querySelector('.log-empty');
    if (empty) empty.remove();

    const statusClass = {
        'success': 'log-status-ok',
        'failed': 'log-status-fail',
        'retry': 'log-status-retry',
        'uploading': 'log-status-upload',
    }[status] || '';

    const statusIcon = {
        'success': '✓',
        'failed': '✗',
        'retry': '⟳',
        'uploading': '↑',
    }[status] || '';

    const entry = document.createElement('div');
    entry.className = 'log-entry';
    entry.innerHTML = `
        <span class="log-time">${time}</span>
        <span class="log-file">${escapeHtml(filename)}</span>
        <span class="${statusClass}">${statusIcon} ${escapeHtml(message)}</span>
    `;

    // Insérer en haut (les plus récents d'abord)
    log.insertBefore(entry, log.firstChild);

    // Limiter à 200 entrées
    while (log.children.length > 200) {
        log.removeChild(log.lastChild);
    }
}

// ─── Stats du dashboard ───

function updateStats(sent, pending, failed, speedKbps) {
    document.getElementById('stat-sent').textContent = sent;
    document.getElementById('stat-pending').textContent = pending;
    document.getElementById('stat-failed').textContent = failed;
    document.getElementById('stat-speed').textContent = speedKbps > 0 ? `~${speedKbps}` : '—';

    const total = sent + pending + failed;
    const percent = total > 0 ? Math.round((sent / total) * 100) : 0;
    document.getElementById('progress-fill').style.width = `${percent}%`;
    document.getElementById('progress-text').textContent = `${sent} / ${total}  (${percent}%)`;

    // Afficher le bouton retry si des fichiers ont échoué
    document.getElementById('dash-retry-btn').style.display = failed > 0 ? 'inline-flex' : 'none';
}
