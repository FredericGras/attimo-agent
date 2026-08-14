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

    // Photo et vidéo tournent ensemble ou séparément (SAAS 430).
    photoEnabled: true,
    videoEnabled: false,
    videoDevicesLoaded: false,
    videoDevice: null,
    videoAudioDevice: null,
    videoFolder: null,
    videoWidth: 1920,
    videoHeight: 1080,
    videoFps: 30,
    videoBitrate: 8,
    videoClipSecs: 150,
    videoOverlapSecs: 30,
    videoSendHd: false,
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
    initVideo();

    // 4. Écouter les événements du backend Rust
    initRustEventListeners();
});

// ═══════════════════════════════════════════════════════
// ÉCRAN LOGIN
// ═══════════════════════════════════════════════════════

function initLogin() {
    // SAAS 240 — Phase 6 (Vague 2C) : authentification par App Password
    // (token "attimo_pat_*" généré depuis le profil sécurité Attimo)
    const form = document.getElementById('login-form');
    const btn = document.getElementById('login-btn');
    const errorDiv = document.getElementById('login-error');

    form.addEventListener('submit', async (e) => {
        e.preventDefault();
        const token = document.getElementById('login-token').value.trim();
        const remember = document.getElementById('login-remember').checked;

        if (!token) return;

        // Afficher le loader
        btn.disabled = true;
        btn.querySelector('.btn-text').style.display = 'none';
        btn.querySelector('.btn-loader').style.display = 'inline-flex';
        errorDiv.style.display = 'none';

        try {
            const result = await invoke('validate_app_password', { token, remember });
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
                    ${event.event_date ? `<span>${escapeHtml(event.event_date)}</span>` : ''}
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

    // Remplir les deux sélecteurs de checkpoint.
    //
    // Photo et vidéo ont chacun le leur : un cameraman peut être posté à un
    // autre point de passage que le photographe.
    const selectPhoto = document.getElementById('config-checkpoint');
    const selectVideo = document.getElementById('config-video-checkpoint');

    selectPhoto.innerHTML = `<option value="">${escapeHtml(t('config.checkpoint_none'))}</option>`;
    selectVideo.innerHTML = `<option value="">${escapeHtml(t('config.video_checkpoint_none'))}</option>`;

    event.checkpoints.forEach(cp => {
        [selectPhoto, selectVideo].forEach(select => {
            const option = document.createElement('option');
            option.value = cp.id;
            option.textContent = cp.name;
            select.appendChild(option);
        });
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

    // Interrupteur du bloc photos
    document.getElementById('config-photo-enabled').addEventListener('change', (e) => {
        AppState.photoEnabled = e.target.checked;
        document.getElementById('config-photo-body')
                .classList.toggle('config-disabled', !e.target.checked);
    });

    initConfigVideo();

    // Bouton démarrer
    document.getElementById('config-start-btn').addEventListener('click', () => {
        const erreur = validerConfig();

        if (erreur) {
            afficherErreurConfig(erreur);
            return;
        }

        afficherErreurConfig(null);
        startSession();
    });
}

function afficherErreurConfig(message) {
    const zone = document.getElementById('config-error');

    if (!message) {
        zone.style.display = 'none';
        return;
    }

    zone.textContent = message;
    zone.style.display = 'block';
}

/**
 * Vérifie que la session peut démarrer.
 *
 * Retourne un message à afficher, ou null si tout est bon. Les contrôles
 * portent sur ce qui rendrait la session inutilisable — pas sur les
 * réglages discutables, qui reçoivent un simple avertissement.
 */
function validerConfig() {
    if (!AppState.photoEnabled && !AppState.videoEnabled) {
        return t('config.nothing_enabled');
    }

    if (AppState.photoEnabled && !AppState.watchFolder) {
        return t('config.folder_required');
    }

    if (AppState.videoEnabled) {
        if (!AppState.videoDevice) {
            return t('config.video_camera_required');
        }

        if (!AppState.videoFolder) {
            return t('config.video_folder_required');
        }
    }

    return null;
}

// ═══════════════════════════════════════════════════════
// CONFIGURATION VIDÉO (SAAS 430)
// ═══════════════════════════════════════════════════════

/**
 * Débit vidéo adapté à la définition choisie, en mégabits par seconde.
 *
 * Un débit fixe ne peut pas convenir à la fois au 720p et à la 4K : trop
 * haut, il gonfle les fichiers pour rien ; trop bas, l'image se délite
 * exactement là où il faudrait lire un dossard.
 *
 * Le coefficient vient du réglage validé sur le terrain — 1080p à 30
 * images par seconde donne 8 Mb/s — et se transpose aux autres
 * définitions. Les bornes évitent les extrêmes absurdes.
 */
function debitPourDefinition(largeur, hauteur, fps) {
    const brut = (largeur * hauteur * fps * 0.13) / 1000000;

    return Math.max(3, Math.min(30, Math.round(brut)));
}

// Intervalle entre deux images d'analyse, en secondes.
const VIDEO_FRAME_INTERVAL = 2.0;

function initConfigVideo() {
    const interrupteur = document.getElementById('config-video-enabled');
    const corps = document.getElementById('config-video-body');

    interrupteur.addEventListener('change', async () => {
        AppState.videoEnabled = interrupteur.checked;
        corps.classList.toggle('config-disabled', !interrupteur.checked);

        // Les périphériques ne sont cherchés qu'à la première activation :
        // l'énumération lance FFmpeg, autant ne pas la faire pour rien.
        if (interrupteur.checked && !AppState.videoDevicesLoaded) {
            await chargerPeripheriquesVideo();
        }
    });

    document.getElementById('config-video-refresh-btn')
            .addEventListener('click', chargerPeripheriquesVideo);

    document.getElementById('config-video-device')
            .addEventListener('change', async (e) => {
        AppState.videoDevice = e.target.value || null;
        await sonderPeripheriqueVideo();
        await rafraichirEstimationDisque();
    });

    document.getElementById('config-video-quality').addEventListener('change', async (e) => {
        appliquerDefinition(e.target.value);
        await rafraichirEstimationDisque();
    });

    document.getElementById('config-video-audio').addEventListener('change', (e) => {
        AppState.videoAudioDevice = e.target.value || null;
        majAvertissementMicro();
        rafraichirEstimationDisque();
    });

    document.getElementById('config-video-browse-btn').addEventListener('click', async () => {
        try {
            const { open } = window.__TAURI__.dialog;
            const selected = await open({
                directory: true,
                multiple: false,
                title: t('config.video_folder_dialog'),
            });

            if (selected) {
                document.getElementById('config-video-folder').value = selected;
                AppState.videoFolder = selected;
                await rafraichirEstimationDisque();
            }
        } catch (e) {
            console.error('Folder dialog error:', e);
        }
    });

    ['config-video-clip', 'config-video-overlap'].forEach(id => {
        document.getElementById(id).addEventListener('input', () => {
            majDecoupage();
            rafraichirEstimationDisque();
        });
    });

    document.getElementById('config-video-hd').addEventListener('change', (e) => {
        AppState.videoSendHd = e.target.checked;
    });

    majDecoupage();
    rafraichirEstimationDisque();
}

/**
 * Interroge Windows sur les caméras et micros branchés.
 */
async function chargerPeripheriquesVideo() {
    const selectCamera = document.getElementById('config-video-device');
    const selectMicro = document.getElementById('config-video-audio');
    const indice = document.getElementById('config-video-device-hint');

    selectCamera.innerHTML = `<option value="">${escapeHtml(t('config.video_devices_loading'))}</option>`;
    indice.textContent = '';

    try {
        const peripheriques = await invoke('list_video_devices');

        const cameras = peripheriques.filter(p => p.kind === 'video');
        const micros = peripheriques.filter(p => p.kind === 'audio');

        selectCamera.innerHTML = '';
        selectMicro.innerHTML = `<option value="">${escapeHtml(t('config.video_micro_none'))}</option>`;

        if (cameras.length === 0) {
            selectCamera.innerHTML = `<option value="">${escapeHtml(t('config.video_devices_none'))}</option>`;
            AppState.videoDevice = null;
        } else {
            cameras.forEach(c => {
                const option = document.createElement('option');
                option.value = c.name;
                option.textContent = c.name;
                selectCamera.appendChild(option);
            });

            AppState.videoDevice = cameras[0].name;
        }

        micros.forEach(m => {
            const option = document.createElement('option');
            option.value = m.name;
            option.textContent = m.name;
            selectMicro.appendChild(option);
        });

        // Le son fait partie du produit : si un micro existe, il est
        // retenu d'office. Le photographe peut toujours le retirer.
        if (micros.length > 0) {
            selectMicro.value = micros[0].name;
            AppState.videoAudioDevice = micros[0].name;
        } else {
            AppState.videoAudioDevice = null;
        }

        AppState.videoDevicesLoaded = true;

        majAvertissementMicro();
        await sonderPeripheriqueVideo();
        await rafraichirEstimationDisque();

    } catch (e) {
        selectCamera.innerHTML = `<option value="">${escapeHtml(t('config.video_devices_none'))}</option>`;
        indice.textContent = String(e);
        indice.classList.add('is-warning');
    }
}

/**
 * Demande à la caméra retenue ce qu'elle sait faire.
 *
 * La résolution n'est pas un réglage exposé : on prend ce que le
 * périphérique propose de mieux dans les limites utiles. Une webcam qui
 * ne monte qu'en 720p doit fonctionner sans que le photographe ait à s'en
 * occuper.
 */
async function sonderPeripheriqueVideo() {
    const indice = document.getElementById('config-video-device-hint');

    if (!AppState.videoDevice) {
        indice.textContent = '';
        return;
    }

    try {
        const capacites = await invoke('probe_video_device', {
            deviceName: AppState.videoDevice
        });

        remplirDefinitions(capacites);

        indice.classList.remove('is-warning');
        indice.textContent = '';

    } catch (e) {
        // Sans réponse du périphérique, on garde les valeurs par défaut :
        // FFmpeg négociera lui-même au lancement.
        indice.classList.add('is-warning');
        indice.textContent = String(e);
    }
}

/**
 * Remplit la liste des définitions proposées par la caméra.
 *
 * Le périphérique remonte souvent la même définition plusieurs fois, une
 * par format brut — yuyv422, bgr24, mjpeg. Ces distinctions ne regardent
 * pas le photographe : on ne garde qu'une entrée par définition, avec la
 * meilleure cadence annoncée.
 */
function remplirDefinitions(capacites) {
    const liste = document.getElementById('config-video-quality');
    const modes = new Map();

    for (const mode of capacites.options) {
        const cle = `${mode.width}x${mode.height}`;
        const connu = modes.get(cle);

        if (!connu || mode.max_fps > connu.max_fps) {
            modes.set(cle, mode);
        }
    }

    // De la plus haute définition à la plus basse : le photographe qui
    // cherche la qualité la trouve en premier.
    const tries = [...modes.values()].sort(
        (a, b) => (b.width * b.height) - (a.width * a.height)
    );

    liste.innerHTML = '';

    for (const mode of tries) {
        // Plafonné à 30 images par seconde, quoi qu'annonce le périphérique.
        //
        // Beaucoup de caméras déclarent 60 sans les tenir à pleine
        // définition : FFmpeg refuse alors d'ouvrir le flux. Et le besoin
        // n'existe pas — on lit des dossards, pas du ralenti — tandis que
        // doubler les images double la charge de l'encodeur sur un portable
        // en extérieur.
        const fps = Math.min(30, Math.round(mode.max_fps));

        const option = document.createElement('option');
        option.value = `${mode.width}x${mode.height}x${fps}`;
        option.textContent = t('config.video_quality_option', {
            width: mode.width,
            height: mode.height,
            fps: fps
        });

        liste.appendChild(option);
    }

    // Par défaut, ce que le backend juge le meilleur compromis.
    const propose = capacites.suggested;

    if (propose) {
        const fps = Math.min(30, Math.round(propose.max_fps));
        liste.value = `${propose.width}x${propose.height}x${fps}`;
    }

    appliquerDefinition(liste.value);
}

/**
 * Retient la définition choisie et en déduit le débit.
 */
function appliquerDefinition(valeur) {
    const indice = document.getElementById('config-video-quality-hint');

    if (!valeur) {
        indice.textContent = '';
        return;
    }

    const [largeur, hauteur, fps] = valeur.split('x').map(Number);

    AppState.videoWidth = largeur;
    AppState.videoHeight = hauteur;
    AppState.videoFps = fps;
    AppState.videoBitrate = debitPourDefinition(largeur, hauteur, fps);

    indice.textContent = t('config.video_quality_hint', {
        rate: AppState.videoBitrate
    });
}

function majAvertissementMicro() {
    const indice = document.getElementById('config-video-audio-hint');

    if (AppState.videoAudioDevice) {
        indice.classList.remove('is-warning');
        indice.textContent = '';
        return;
    }

    indice.classList.add('is-warning');
    indice.textContent = t('config.video_micro_warning');
}

/**
 * Affiche le découpage qui découlera des réglages.
 *
 * Le photographe saisit une durée de clip et un recouvrement ; l'agent en
 * déduit une taille de morceau. Certaines combinaisons donnent des
 * morceaux d'une seconde et trente fichiers par clip — techniquement
 * valides, mais absurdes en pratique. Elles reçoivent un avertissement
 * plutôt qu'un refus : la souplesse reste au photographe.
 */
function majDecoupage() {
    const zone = document.getElementById('config-video-plan');

    const clip = parseInt(document.getElementById('config-video-clip').value, 10);
    const recouvrement = parseInt(document.getElementById('config-video-overlap').value, 10);

    AppState.videoClipSecs = clip;
    AppState.videoOverlapSecs = recouvrement;

    if (!clip || !recouvrement || recouvrement >= clip) {
        zone.classList.add('is-warning');
        zone.textContent = t('config.video_split_invalid');
        return;
    }

    const pas = clip - recouvrement;
    const morceau = pgcdJs(clip, pas);
    const parClip = clip / morceau;

    // Au-delà d'une dizaine de morceaux par clip, on multiplie les
    // fichiers et les ouvertures disque sans rien y gagner.
    if (morceau < 5 || parClip > 12) {
        zone.classList.add('is-warning');
        zone.textContent = t('config.video_split_warning', {
            segment: morceau,
            count: parClip
        });
        return;
    }

    zone.classList.remove('is-warning');
    zone.textContent = t('config.video_split_ok', {
        step: pas,
        segment: morceau,
        count: parClip
    });
}

function pgcdJs(a, b) {
    return b === 0 ? a : pgcdJs(b, a % b);
}

/**
 * Demande au backend combien de temps le disque peut tenir.
 *
 * Quatre flux s'écrivent en parallèle pour la même seconde captée —
 * morceaux, clips, versions légères et images d'analyse — et rien n'est
 * supprimé. L'ordre de grandeur n'est donc pas celui qu'on devine.
 */
async function rafraichirEstimationDisque() {
    const bloc = document.getElementById('config-video-disk');
    const principal = document.getElementById('config-video-disk-main');
    const secondaire = document.getElementById('config-video-disk-sub');

    if (!AppState.videoFolder || !AppState.videoDevice) {
        bloc.classList.remove('is-warning');
        principal.textContent = t('config.disk_pending');
        secondaire.textContent = '';
        return;
    }

    try {
        const estimation = await invoke('disk_estimate', {
            config: construireConfigVideo(),
            proxyBitrateMbps: null,
            intervalSecs: VIDEO_FRAME_INTERVAL
        });

        const goParHeure = (estimation.total_per_hour / 1073741824).toFixed(1);
        const heures = Math.floor(estimation.autonomy_secs / 3600);
        const libres = Math.round(estimation.free_bytes / 1073741824);
        const reserve = Math.round(estimation.reserve_bytes / 1073741824);

        principal.innerHTML = escapeHtml(t('config.disk_rate', { rate: goParHeure }))
                            + ' — <strong>' + escapeHtml(t('config.disk_hours', { hours: heures })) + '</strong>';

        secondaire.textContent = t('config.disk_detail', {
            free: libres,
            reserved: reserve
        });

        // Sous six heures, une course de la journée ne tient pas.
        bloc.classList.toggle('is-warning', heures < 6);

    } catch (e) {
        bloc.classList.add('is-warning');
        principal.textContent = String(e);
        secondaire.textContent = '';
    }
}

/**
 * Assemble les réglages de captation attendus par le backend.
 */
function construireConfigVideo() {
    return {
        device_name: AppState.videoDevice || '',
        audio_device: AppState.videoAudioDevice,
        width: AppState.videoWidth,
        height: AppState.videoHeight,
        fps: AppState.videoFps,
        bitrate_mbps: AppState.videoBitrate,
        clip_duration_secs: AppState.videoClipSecs,
        overlap_secs: AppState.videoOverlapSecs,
        output_dir: AppState.videoFolder || ''
    };
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

    // La file d'envoi vidéo tourne dès l'ouverture d'une session, même sans
    // captation : elle peut avoir des clips d'une sortie précédente à
    // écouler, et elle ne traite que l'événement ouvert.
    demarrerFileVideo();

    // ── Captation vidéo (SAAS 431) ──
    if (AppState.videoEnabled) {
        await demarrerCaptation();
    }

    // ── Appeler le backend Rust pour démarrer la surveillance photo ──
    if (!AppState.photoEnabled) {
        return;
    }

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
    // ── Retour aux réglages, sans rien interrompre ──
    //
    // Indispensable pour ajouter la vidéo alors que les photos tournent
    // déjà, ou l'inverse. La session continue derrière.
    document.getElementById('dash-back-btn').addEventListener('click', () => {
        navigateTo('config');
    });

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
            // La captation d'abord : elle doit clore son manifeste pendant
            // que la session est encore ouverte.
            await arreterCaptation();
            arreterFileVideo();

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

// ═══════════════════════════════════════════════════════════════════════
// CAPTATION VIDÉO (SAAS 431)
// ═══════════════════════════════════════════════════════════════════════
//
// Le photographe filme un point de passage. FFmpeg produit des morceaux
// courts ; l'assembleur les colle en clips qui se chevauchent, sans jamais
// réencoder. Chaque morceau donne aussi des images d'analyse, d'où les
// dossards seront lus.
//
// Tout part de l'écran de réglages : aucune valeur n'est écrite en dur.

let videoSessionId = null;
let videoDebutMs = 0;
let videoClips = 0;
let videoImages = 0;
let videoChronoTimer = null;
let videoFileTimer = null;

/**
 * Branche les écoutes du backend.
 *
 * Une seule fois au lancement : ces abonnements survivent aux captations
 * successives, et en créer un par session les empilerait.
 */
async function initVideo() {
    await listen('segment-ready', surMorceauPret);
    await listen('disk-status', surEtatDisque);

    document.getElementById('dash-video-stop-btn').addEventListener('click', () => {
        arreterCaptation();
    });
}

/**
 * Lance la captation avec les réglages de l'écran.
 */
async function demarrerCaptation() {
    const evenement = AppState.selectedEvent;
    const selectCp = document.getElementById('config-video-checkpoint');
    const checkpointId = selectCp.value ? parseInt(selectCp.value, 10) : null;

    videoSessionId = 'video_' + Date.now();
    videoDebutMs = Date.now();
    videoClips = 0;
    videoImages = 0;

    try {
        // L'autorisation d'envoi HD est un état du backend : il faut la
        // poser avant que le premier clip n'entre en file.
        await invoke('set_hd_upload', { allow: AppState.videoSendHd });

        const plan = await invoke('start_recording', {
            sessionId: videoSessionId,
            attimoTenantId: AppState.user ? AppState.user.id : 0,
            attimoEventId: evenement.id,
            attimoCheckpointId: checkpointId,
            intervalSecs: VIDEO_FRAME_INTERVAL,
            config: construireConfigVideo()
        });

        document.getElementById('dash-video-panel').style.display = 'block';
        majCompteursVideo();

        if (!videoChronoTimer) {
            videoChronoTimer = setInterval(majChronoVideo, 1000);
        }

        addLogEntry(timeNow(), '—', 'success',
            t('dashboard.video_started', { step: plan.step_secs }));

        return true;

    } catch (e) {
        videoSessionId = null;
        addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error: e }));
        return false;
    }
}

/**
 * Arrête la captation sans toucher à la surveillance des photos.
 *
 * Les fichiers déjà produits restent en file : couper la caméra ne doit
 * pas annuler ce qui attend d'être envoyé.
 */
async function arreterCaptation() {
    if (!videoSessionId) {
        return;
    }

    // L'identifiant est figé maintenant. Le traitement d'un morceau dure
    // plusieurs secondes — extraction, assemblage, réencodage — et l'arrêt
    // peut tomber pendant. Sans cette copie, la fin du traitement
    // travaillerait sous une session déjà refermée.
    const sessionEnCours = videoSessionId;

    try {
        await invoke('stop_recording');
    } catch (e) {
        console.error('Stop recording error:', e);
    }

    addLogEntry(timeNow(), '—', 'retry',
        t('dashboard.video_stopped', { clips: videoClips }));

    videoSessionId = null;

    if (videoChronoTimer) {
        clearInterval(videoChronoTimer);
        videoChronoTimer = null;
    }

    document.getElementById('dash-video-panel').style.display = 'none';
}

/**
 * Un morceau vient d'être clos par FFmpeg.
 *
 * C'est le signal que le fichier est complet : avant, il est encore en
 * cours d'écriture et l'ouvrir donnerait une vidéo tronquée.
 */
async function surMorceauPret(evt) {
    const morceau = evt.payload;

    // FFmpeg met un instant à se fermer : il clôt le morceau en cours et
    // l'annonce après l'arrêt. Sans cette garde, on tenterait de le mettre
    // en file sous une session qui n'existe plus.
    //
    // Le fichier n'est pas perdu pour autant — il reste sur le disque, et
    // le morceau incomplet n'avait de toute façon pas sa place dans un clip.
    if (!videoSessionId) {
        return;
    }

    // L'identifiant est figé maintenant. Le traitement d'un morceau dure
    // plusieurs secondes — extraction, assemblage, réencodage — et l'arrêt
    // peut tomber pendant. Sans cette copie, la fin du traitement
    // travaillerait sous une session déjà refermée.
    const sessionEnCours = videoSessionId;

    // Images d'analyse — extraites du MORCEAU, jamais du clip.
    //
    // Les clips se chevauchent : extraire depuis eux traiterait deux fois
    // les mêmes instants, et ferait payer deux fois la reconnaissance pour
    // un résultat identique.
    try {
        const res = await invoke('extract_frames', {
            segmentIndex: morceau.index,
            intervalSecs: VIDEO_FRAME_INTERVAL
        });

        videoImages += res.frames.length;
        majCompteursVideo();

        // Mise en file plutôt qu'envoi direct : c'est ce qui permet de
        // couper l'envoi vidéo sans rien perdre, et de le reprendre plus
        // tard — y compris après avoir fermé l'agent.
        for (const image of res.frames) {
            await invoke('queue_video_file', {
                sessionId: sessionEnCours,
                eventId: AppState.selectedEvent.id,
                kind: 'frames',
                filePath: image.path,
                clipIndex: null,
                startedAt: null,
                endedAt: null,
                instantAt: image.instant_at
            });
        }
    } catch (e) {
        addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error: e }));
    }

    // Chaque morceau peut compléter un clip. On demande à l'assembleur.
    try {
        const clip = await invoke('on_segment_ready', { segmentIndex: morceau.index });

        if (!clip) {
            return;
        }

        videoClips++;
        majCompteursVideo();

        addLogEntry(timeNow(), clip.filename, 'success',
            t('dashboard.video_clip_ready', { index: clip.index }));

        // Version légère : c'est elle que le coureur regarde pendant la
        // course, le fichier lourd pouvant monter le soir.
        const proxyPath = await invoke('generate_proxy', {
            clipPath: clip.path,
            height: 540,
            bitrateMbps: 2
        });

        // Horodatages du clip, dérivés du départ de la session.
        const debutMs = videoDebutMs + clip.offset_secs * 1000;
        const finMs = debutMs + clip.duration_secs * 1000;
        const iso = (ms) => new Date(ms).toISOString();

        // Les deux variantes entrent en file. L'ordre d'envoi réel dépendra
        // de la priorité et de l'autorisation HD, pas de l'ordre d'ajout.
        await invoke('queue_video_file', {
            sessionId: sessionEnCours,
            eventId: AppState.selectedEvent.id,
            kind: 'clip_proxy',
            filePath: proxyPath,
            clipIndex: clip.index,
            startedAt: iso(debutMs),
            endedAt: iso(finMs),
            instantAt: null
        });

        await invoke('queue_video_file', {
            sessionId: sessionEnCours,
            eventId: AppState.selectedEvent.id,
            kind: 'clip_hd',
            filePath: clip.path,
            clipIndex: clip.index,
            startedAt: iso(debutMs),
            endedAt: iso(finMs),
            instantAt: null
        });

    } catch (e) {
        addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error: e }));
    }
}

/**
 * La surveillance disque a parlé.
 *
 * Elle tourne côté Rust toutes les quinze secondes et arrête la captation
 * d'elle-même avant saturation. Ici on ne fait qu'afficher.
 */
function surEtatDisque(evt) {
    const etat = evt.payload;

    const bloc = document.getElementById('dash-video-disk');
    const autonomie = document.getElementById('dash-video-disk-autonomy');
    const detail = document.getElementById('dash-video-disk-detail');
    const jauge = document.getElementById('dash-video-disk-fill');

    const heures = Math.floor(etat.autonomy_secs / 3600);
    const minutes = Math.round(etat.autonomy_secs / 60);

    autonomie.textContent = heures >= 1
        ? t('dashboard.video_disk_autonomy', { hours: heures })
        : t('dashboard.video_disk_minutes', { minutes: minutes });

    const ecrits = (etat.written_bytes / 1073741824).toFixed(1);
    const debit = (etat.rate_bytes_per_sec * 3600 / 1073741824).toFixed(1);

    detail.textContent = etat.rate_measured
        ? t('dashboard.video_disk_detail', { written: ecrits, rate: debit })
        : t('dashboard.video_disk_estimated', { written: ecrits, rate: debit });

    // La jauge montre la part consommée du disque utilisable.
    const total = etat.written_bytes + etat.usable_bytes;
    const part = total > 0 ? (etat.written_bytes / total) * 100 : 0;
    jauge.style.width = Math.min(100, Math.round(part)) + '%';

    bloc.classList.toggle('is-warning', etat.level === 'warning');
    bloc.classList.toggle('is-critical',
        etat.level === 'critical' || etat.level === 'stopped');

    // Le backend a coupé la captation : l'écran doit suivre.
    if (etat.level === 'stopped') {
        addLogEntry(timeNow(), '—', 'failed', t('dashboard.video_disk_full'));
        videoSessionId = null;

        if (videoChronoTimer) {
            clearInterval(videoChronoTimer);
            videoChronoTimer = null;
        }
    }
}

function majChronoVideo() {
    if (!videoSessionId) {
        return;
    }

    const secondes = Math.floor((Date.now() - videoDebutMs) / 1000);

    const h = String(Math.floor(secondes / 3600)).padStart(2, '0');
    const m = String(Math.floor((secondes % 3600) / 60)).padStart(2, '0');
    const s = String(secondes % 60).padStart(2, '0');

    document.getElementById('dash-video-elapsed').textContent = `${h}:${m}:${s}`;
}

function majCompteursVideo() {
    document.getElementById('dash-video-clips').textContent = videoClips;
    document.getElementById('dash-video-frames').textContent = videoImages;
}

// ═══════════════════════════════════════════════════════════════════════
// FILE D'ENVOI VIDÉO
// ═══════════════════════════════════════════════════════════════════════
//
// Vide la file en continu, un élément à la fois.
//
// Le rythme d'un élément par seconde évite de saturer le réseau et laisse
// le temps d'interrompre proprement : couper l'envoi vidéo prend effet au
// fichier suivant, pas au bout de trois clips.

function demarrerFileVideo() {
    if (!videoFileTimer) {
        videoFileTimer = setInterval(traiterFileVideo, 1000);
    }
}

function arreterFileVideo() {
    if (videoFileTimer) {
        clearInterval(videoFileTimer);
        videoFileTimer = null;
    }
}

async function traiterFileVideo() {
    if (!AppState.selectedEvent) {
        return;
    }

    try {
        const resultat = await invoke('process_video_queue', {
            token: AppState.token,
            eventId: AppState.selectedEvent.id,
            checkpointId: null
        });

        if (resultat && resultat.detail && resultat.detail.kind === 'clip') {
            const clip = resultat.detail.clip;
            addLogEntry(timeNow(), '—', 'success',
                t('dashboard.video_clip_sent', { index: clip.index }));
        }

        if (resultat && resultat.detail && resultat.detail.totals) {
            const totaux = resultat.detail.totals;

            if (totaux.bibs > 0) {
                addLogEntry(timeNow(), '—', 'success',
                    t('dashboard.video_bibs_read', { count: totaux.bibs }));
            }
        }

    } catch (e) {
        if (e === 'SESSION_EXPIRED') {
            arreterFileVideo();
        }
    }

    await rafraichirFileVideo();
}

async function rafraichirFileVideo() {
    if (!AppState.selectedEvent) {
        return;
    }

    try {
        const stats = await invoke('video_queue_stats', {
            eventId: AppState.selectedEvent.id
        });

        const total = stats.frames_pending + stats.proxy_pending + stats.hd_pending;

        document.getElementById('dash-video-queue').textContent = total;

        const mo = (stats.total_bytes / 1048576).toFixed(0);

        document.getElementById('dash-video-queue-label').textContent =
            total > 0
                ? t('dashboard.video_queue_size', { size: mo })
                : t('dashboard.video_queue');

    } catch (e) {
        // Sans importance : l'affichage se rafraîchira au prochain passage.
    }
}