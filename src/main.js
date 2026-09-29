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
    // 3 par défaut (0.3.1) : le serveur répond dès la réception, et 3
    // envois suffisent déjà à remplir une 4G.
    parallelUploads: 3,
    includeExisting: true,
    isPaused: false,

    // Épreuve de la session qui tourne (0.3.1). Distincte de selectedEvent,
    // celle que l'on règle : on peut désormais parcourir les autres écrans
    // — et même ouvrir une autre épreuve — sans couper la session.
    activeEvent: null,

    // Réglages de la surveillance photo en cours : les revalider à
    // l'identique ne doit pas la relancer.
    photoReglages: null,

    // Session ouverte pour le seul envoi des clips HD (0.3.1).
    envoiSeul: false,

    // Derniers compteurs photo, pour le bandeau de session.
    photoStats: { sent: 0, pending: 0, failed: 0 },

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

    majBandeauSession();
}

/**
 * Une session tourne-t-elle ? Photos, captation, ou envoi des clips HD.
 */
function sessionEnCours() {
    return AppState.activeEvent !== null;
}

function dormir(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

/**
 * Demande une confirmation au photographe (0.3.2).
 *
 * Le module de dialogue de Tauri remplace window.confirm par une version
 * ASYNCHRONE, qui rend une promesse. Écrit « if (!confirm(...)) », le test
 * ne valait rien : une promesse est toujours « vraie », la suite partait
 * sans attendre la réponse, et sans en tenir compte. D'où « Surveillance
 * démarrée » au journal avant même le oui — et un non qui n'arrêtait rien.
 *
 * Attendre la réponse fonctionne dans les deux cas : un booléen attendu
 * reste un booléen.
 */
async function confirmer(message) {
    try {
        return (await window.confirm(message)) === true;
    } catch (e) {
        console.error('Confirm error:', e);
        return false;
    }
}

// ─── Initialisation au lancement ───

document.addEventListener('DOMContentLoaded', async () => {
    // 1. Charger les traductions AVANT tout le reste
    await initI18n();

    afficherVersion();

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
    initBandeauSession();
    initVideo();

    // 4. Écouter les événements du backend Rust
    initRustEventListeners();
});

/**
 * Version et édition de l'agent (0.3.1).
 *
 * L'agent de DEV s'installe à côté de celui de production : le photographe
 * doit voir d'un coup d'œil lequel il a ouvert.
 */
async function afficherVersion() {
    try {
        const info = await invoke('agent_info');
        const edition = info.edition ? ` ${String(info.edition).toUpperCase()}` : '';

        document.querySelectorAll('.app-version').forEach(el => {
            el.textContent = `v${info.version}${edition}`;
        });

        if (info.edition) {
            document.body.classList.add('is-dev');
            document.title = `Attimo Agent Terrain${edition}`;
        }
    } catch (e) {
        console.error('Agent info error:', e);
    }
}

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

            // Reconnexion après expiration : la file vidéo s'était arrêtée
            // faute de jeton valide, elle repart avec le nouveau.
            if (sessionEnCours()) {
                demarrerFileVideo();
            }

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
    // Le journal s'exporte aussi hors session (0.3.2) : après une course,
    // la surveillance n'est plus accessible, le diagnostic doit l'être.
    document.getElementById('events-export-log-btn').addEventListener('click', exporterJournal);

    document.getElementById('logout-btn').addEventListener('click', async () => {
        // Se déconnecter coupe les envois : on le dit, et on ferme proprement.
        if (sessionEnCours()) {
            if (!(await confirmer(t('nav.confirm_logout')))) {
                return;
            }

            await terminerSession();
        }

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

            // Vidéos en ligne et mode d'identification (0.3.3) : affichés
            // seulement si le serveur les donne. Il ne le fait pas encore.
            const videosLabel = Affichage.videosEnLigne(event.video_count, tPlural);
            const modeLabel = Affichage.modeIdentification(event.recognition_mode, t);

            const liveHtml = event.is_live
                ? '<span class="event-live">● LIVE</span>'
                : '';

            // L'épreuve dont la session tourne : un clic ramène à la
            // surveillance, sans rien relancer.
            const enCours = sessionEnCours() && AppState.activeEvent.id === event.id;

            const runningHtml = enCours
                ? `<span class="event-running">${escapeHtml(t('nav.event_running'))}</span>`
                : '';

            const card = document.createElement('div');
            card.className = 'event-card' + (enCours ? ' is-running' : '');
            card.innerHTML = `
                <div class="event-card-header">
                    <span class="event-icon">${icon}</span>
                    <span class="event-name">${escapeHtml(event.name)}</span>
                    ${liveHtml}
                    ${runningHtml}
                </div>
                <div class="event-meta">
                    ${event.event_date ? `<span>${escapeHtml(event.event_date)}</span>` : ''}
                    <span>${escapeHtml(photoLabel)}</span>
                    ${videosLabel ? `<span>${escapeHtml(videosLabel)}</span>` : ''}
                    <span>${escapeHtml(cpLabel)}</span>
                    ${modeLabel ? `<span>${escapeHtml(modeLabel)}</span>` : ''}
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
    // Épreuve de la session en cours : retour direct à la surveillance.
    if (sessionEnCours() && AppState.activeEvent.id === event.id) {
        navigateTo('dashboard');
        return;
    }

    remplirConfig(event);

    // Reset du dossier
    document.getElementById('config-folder').value = '';
    AppState.watchFolder = null;
    majAlerteDossiers();

    majHdEnAttenteConfig(event);

    navigateTo('config');
}

/**
 * Réglages de la session en cours (0.3.1).
 *
 * Le photographe a pu ouvrir une autre épreuve entre-temps : l'écran de
 * réglages est alors rempli pour elle. On le remet sur l'épreuve de la
 * session, avec le dossier et les checkpoints qui tournent.
 */
function ouvrirReglagesSession() {
    const evenement = AppState.activeEvent;

    if (evenement && (!AppState.selectedEvent || AppState.selectedEvent.id !== evenement.id)) {
        remplirConfig(evenement);

        const reglages = AppState.photoReglages;

        document.getElementById('config-folder').value = reglages ? reglages.folder : '';
        AppState.watchFolder = reglages ? reglages.folder : null;

        if (reglages && reglages.checkpointId) {
            document.getElementById('config-checkpoint').value = String(reglages.checkpointId);
        }

        if (videoCheckpointId) {
            document.getElementById('config-video-checkpoint').value = String(videoCheckpointId);
        }

        document.getElementById('config-hd-card').style.display = 'none';
    }

    majAlerteDossiers();
    navigateTo('config');
}

/**
 * Remplit l'écran de réglages pour une épreuve.
 */
function remplirConfig(event) {
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
}

// ═══════════════════════════════════════════════════════
// ÉCRAN CONFIGURATION
// ═══════════════════════════════════════════════════════

function initConfig() {
    // Bouton retour
    document.getElementById('config-back-btn').addEventListener('click', () => {
        navigateTo('events');
        loadEvents();
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
                majAlerteDossiers();
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
        majAlerteDossiers();
    });

    initConfigVideo();

    // Envoi des clips HD d'une sortie précédente (0.3.1)
    document.getElementById('config-hd-send-btn').addEventListener('click', demarrerEnvoiHdSeul);

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

/**
 * Deux dossiers se recouvrent-ils ? Identiques, ou l'un dans l'autre
 * (0.3.2). Windows ne distingue pas les majuscules, ni / de \.
 */
function dossiersSeRecouvrent(a, b) {
    if (!a || !b) {
        return false;
    }

    const normal = (chemin) => String(chemin).replace(/\\/g, '/').replace(/\/+$/, '').toLowerCase() + '/';
    const na = normal(a);
    const nb = normal(b);

    return na.startsWith(nb) || nb.startsWith(na);
}

/**
 * Avertit, sous le dossier d'enregistrement vidéo, s'il se confond avec le
 * dossier photos surveillé (0.3.2).
 */
function majAlerteDossiers() {
    const indice = document.getElementById('config-video-folder-hint');

    if (AppState.photoEnabled && AppState.videoEnabled
        && dossiersSeRecouvrent(AppState.watchFolder, AppState.videoFolder)) {
        indice.textContent = t('config.video_folder_same_as_photos');
        indice.classList.add('is-warning');
        indice.style.display = '';
    } else {
        indice.textContent = '';
        indice.classList.remove('is-warning');
        indice.style.display = 'none';
    }
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

/**
 * Clips HD restés sur le disque pour cette épreuve (0.3.1).
 *
 * Le photographe qui rentre le soir ouvre l'épreuve et voit tout de suite
 * ce qui reste à envoyer, avec un bouton pour le faire — sans avoir à
 * relancer une captation ni à chercher les fichiers.
 */
async function majHdEnAttenteConfig(event) {
    const carte = document.getElementById('config-hd-card');
    carte.style.display = 'none';

    // Un clip supprimé du disque ne doit pas être annoncé « en attente »
    // (0.3.2) : il sort de la file d'abord.
    await purgerFichiersVideoAbsents(event.id);

    try {
        const stats = await invoke('video_queue_stats', { eventId: event.id, sessions: null });
        const restant = stats.hd_pending + stats.hd_sending;

        // L'épreuve a pu changer pendant l'appel.
        if (!AppState.selectedEvent || AppState.selectedEvent.id !== event.id || restant === 0) {
            return;
        }

        document.getElementById('config-hd-text').textContent = t('config.hd_pending_text', {
            count: restant,
            size: (stats.hd_bytes_pending / 1073741824).toFixed(1)
        });

        carte.style.display = 'block';
    } catch (e) {
        console.error('HD stats error:', e);
    }
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
        majAlerteDossiers();

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
                majAlerteDossiers();
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

    document.getElementById('config-video-hd').addEventListener('change', async (e) => {
        AppState.videoSendHd = e.target.checked;

        // Pendant une session, le choix s'applique tout de suite : c'est le
        // même réglage que le bouton « Envoyer les HD maintenant ».
        if (sessionEnCours()) {
            try {
                await invoke('set_hd_upload', { allow: AppState.videoSendHd });
            } catch (err) {
                console.error('HD toggle error:', err);
            }

            majPanneauVideo();
        }
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

/**
 * Remet le tableau de bord à zéro pour une nouvelle session.
 */
function preparerTableauDeBord(event) {
    document.getElementById('dash-event-name').textContent = event.name;

    // Reset des stats
    AppState.photoStats = { sent: 0, pending: 0, failed: 0 };
    document.getElementById('stat-sent').textContent = '0';
    document.getElementById('stat-pending').textContent = '0';
    document.getElementById('stat-failed').textContent = '0';
    document.getElementById('stat-speed').textContent = '—';
    document.getElementById('progress-fill').style.width = '0%';
    document.getElementById('progress-text').textContent = '0 / 0';
    document.getElementById('dash-retry-btn').style.display = 'none';
    document.getElementById('upload-log').innerHTML = `<div class="log-empty">${escapeHtml(t('dashboard.log_empty'))}</div>`;

    // Lignes écrites avant la session (fichiers vidéo retirés à
    // l'ouverture de l'épreuve) : reprises ici, déjà copiées sur le disque,
    // et rappelées dans le bloc vidéo (0.3.3).
    videoAvis = null;
    videoDebitHd = null;

    // Le bilan HD repart de zéro (0.3.4) : « 5 envoyés — 8 introuvables »
    // restait affiché à l'ouverture de la session suivante. Les clips
    // retirés à l'ouverture de l'épreuve, juste avant, font partie de cette
    // session : le bilan part de leur retrait.
    videoHdDepuisMs = Date.now();

    for (const ligne of journalAvantSession.splice(0)) {
        ajouterLigneEcran(ligne.heure, '—', 'retry', ligne.message);
        videoAvis = ligne.message;
        videoHdDepuisMs = Math.min(videoHdDepuisMs, ligne.ms);
    }
    debitFenetre.length = 0;

    // Reset de l'état pause
    AppState.isPaused = false;
    majBoutonPausePhotos();
    document.getElementById('dash-status').className = 'status-dot status-active';

    // Vidéo : compteurs de la nouvelle session
    videoSessionsCourantes = [];
    videoImages = 0;
    videoActivite = false;
    videoStatsEvenement = null;
    videoStatsSession = null;

    // 0.3.2 — Chrono et espace disque repartent de zéro : ils affichaient
    // ceux de la session précédente (« 00:09:25 », « 1,3 Go écrits ») au
    // démarrage d'une nouvelle. Les reprises d'une même session, elles,
    // cumulent.
    videoCumulMs = 0;
    videoDebutMs = 0;
    videoDisqueParCaptation = {};
    videoAnalyseDesactivee = false;
    videoAnalyseAnnoncee = false;
    document.getElementById('dash-video-elapsed').textContent = '00:00:00';
    document.getElementById('dash-video-disk-autonomy').textContent = '';
    document.getElementById('dash-video-disk-detail').textContent = '';
    document.getElementById('dash-video-disk-fill').style.width = '0%';
    document.getElementById('dash-video-disk').classList.remove('is-warning', 'is-critical');

    majPanneauVideo();
}

async function startSession() {
    const event = AppState.selectedEvent;
    const cpSelect = document.getElementById('config-checkpoint');
    const checkpointId = cpSelect.value ? parseInt(cpSelect.value) : null;
    const checkpointName = cpSelect.value ? cpSelect.options[cpSelect.selectedIndex].text : null;

    // Une autre épreuve tourne : on ne la coupe que si le photographe le
    // demande explicitement.
    if (sessionEnCours() && AppState.activeEvent.id !== event.id) {
        if (!(await confirmer(t('nav.confirm_replace', { event: AppState.activeEvent.name })))) {
            return;
        }

        await terminerSession();
    }

    // Même épreuve, session déjà ouverte : on la complète (ajouter la vidéo
    // aux photos, changer le parallélisme…) sans rien recommencer.
    const complement = sessionEnCours();

    if (!complement) {
        preparerTableauDeBord(event);
    }

    AppState.activeEvent = event;
    AppState.envoiSeul = false;

    if (AppState.photoEnabled) {
        AppState.selectedCheckpoint = checkpointId ? { id: checkpointId, name: checkpointName } : null;
        document.getElementById('dash-checkpoint-name').textContent = checkpointName || '';
        document.getElementById('dash-checkpoint-name').style.display = checkpointName ? 'inline-block' : 'none';
        document.getElementById('dash-folder').textContent = AppState.watchFolder;
        document.getElementById('dash-folder').title = AppState.watchFolder;
    }

    navigateTo('dashboard');

    // La file d'envoi vidéo tourne dès l'ouverture d'une session, même sans
    // captation : elle peut avoir des clips d'une sortie précédente à
    // écouler, et elle ne traite que l'événement ouvert.
    demarrerFileVideo();

    // Clips dont le fichier a disparu du disque : hors de la file (0.3.2).
    purgerFichiersVideoAbsents(event.id);

    // Même dossier pour la vidéo et les photos (0.3.2) : sans danger, mais à
    // signaler — on s'y retrouve mal, et le photographe ne l'a peut-être pas
    // voulu.
    if (AppState.photoEnabled && AppState.videoEnabled
        && dossiersSeRecouvrent(AppState.watchFolder, AppState.videoFolder)) {
        addLogEntry(timeNow(), '—', 'retry', t('config.video_folder_same_as_photos'));
    }

    // ── Captation vidéo (SAAS 431) ──
    if (AppState.videoEnabled) {
        // Une captation déjà ouverte — en cours ou en pause — continue.
        if (!captationOuverte()) {
            await demarrerCaptation(false);
        }
    } else if (captationOuverte()) {
        // Le photographe a éteint la vidéo dans les réglages.
        await arreterCaptation();
    }

    majPanneauVideo();

    // ── Surveillance photo ──
    if (!AppState.photoEnabled) {
        if (AppState.sessionId) {
            await arreterPhotos();
        }

        majBandeauSession();
        return;
    }

    const reglages = {
        folder: AppState.watchFolder,
        checkpointId: checkpointId,
    };

    // Même dossier, même checkpoint : la surveillance continue. Seul le
    // nombre d'envois simultanés change, à chaud — relancer la session
    // renverrait toutes les photos du dossier.
    if (AppState.sessionId && AppState.photoReglages
        && AppState.photoReglages.folder === reglages.folder
        && AppState.photoReglages.checkpointId === reglages.checkpointId) {
        try {
            await invoke('set_parallel', { parallel: AppState.parallelUploads });
            addLogEntry(timeNow(), '—', 'success',
                t('dashboard.msg_parallel', { count: AppState.parallelUploads }));
        } catch (e) {
            addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error: e }));
        }

        majBandeauSession();
        return;
    }

    // ── Appeler le backend Rust pour démarrer la surveillance photo ──
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
        AppState.photoReglages = reglages;
        addLogEntry(timeNow(), '—', 'success', result.message);

    } catch (error) {
        addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error }));
    }

    majBoutonPausePhotos();
    majBandeauSession();
}

/**
 * Arrête la seule surveillance photo.
 */
async function arreterPhotos() {
    try {
        await invoke('stop_session');
        addLogEntry(timeNow(), '—', 'retry', t('dashboard.msg_stopped'));
    } catch (e) {
        console.error('Stop error:', e);
    }

    AppState.sessionId = null;
    AppState.photoReglages = null;
    AppState.isPaused = false;
    majBoutonPausePhotos();
}

/**
 * Ferme la session : captation, photos, puis vidage de la file vidéo.
 *
 * La captation d'abord : elle doit clore son manifeste pendant que la
 * session est encore ouverte. La file n'est pas coupée : elle écoule ce
 * qui reste de l'épreuve, puis s'arrête seule.
 */
async function terminerSession() {
    await arreterCaptation();
    demarrerVidageFinal();
    await arreterPhotos();

    AppState.activeEvent = null;
    AppState.envoiSeul = false;
    videoSessionsCourantes = [];
    videoActivite = false;

    majPanneauVideo();
    majBandeauSession();
}

/**
 * Envoi des clips HD sans captation ni photos (0.3.1).
 *
 * Le parcours du soir : ouvrir l'épreuve, « Envoyer les HD maintenant »,
 * suivre l'avancement. Plus besoin de surveiller le dossier ni de cocher
 * « envoyer la pleine qualité pendant la course ».
 */
async function demarrerEnvoiHdSeul() {
    const event = AppState.selectedEvent;

    if (sessionEnCours() && AppState.activeEvent.id !== event.id) {
        if (!(await confirmer(t('nav.confirm_replace', { event: AppState.activeEvent.name })))) {
            return;
        }

        await terminerSession();
    }

    if (!sessionEnCours()) {
        preparerTableauDeBord(event);
        AppState.activeEvent = event;
        AppState.envoiSeul = true;

        document.getElementById('dash-checkpoint-name').style.display = 'none';
        document.getElementById('dash-folder').textContent = '';
    }

    // Les clips supprimés du disque sortent de la file avant l'envoi : aucun
    // ne doit échouer au clic (0.3.2).
    await purgerFichiersVideoAbsents(event.id);

    await activerEnvoiHd(true);

    navigateTo('dashboard');
    demarrerFileVideo();
    majPanneauVideo();
}

// ═══════════════════════════════════════════════════════
// ÉCRAN DASHBOARD
// ═══════════════════════════════════════════════════════

function initDashboard() {
    // ── Retour aux réglages, sans rien interrompre ──
    //
    // Indispensable pour ajouter la vidéo alors que les photos tournent
    // déjà, ou l'inverse. La session continue derrière.
    document.getElementById('dash-back-btn').addEventListener('click', ouvrirReglagesSession);

    // ── Accueil, sans rien interrompre (0.3.1) ──
    //
    // La captation et les envois continuent ; le bandeau de session permet
    // de revenir ici depuis n'importe quel écran.
    document.getElementById('dash-home-btn').addEventListener('click', () => {
        navigateTo('events');
        loadEvents();
    });

    // ── Bouton Pause / Reprendre ──
    document.getElementById('dash-pause-btn').addEventListener('click', async () => {
        const dot = document.getElementById('dash-status');

        try {
            if (!AppState.isPaused) {
                // Mettre en pause
                await invoke('pause_session');
                AppState.isPaused = true;
                dot.className = 'status-dot status-paused';
                addLogEntry(timeNow(), '—', 'retry', t('dashboard.msg_paused'));
            } else {
                // Reprendre. L'agent revérifie aussi la galerie (0.3.4) :
                // une photo supprimée pendant la pause repart.
                await invoke('resume_session');
                AppState.isPaused = false;
                dot.className = 'status-dot status-active';
                addLogEntry(timeNow(), '—', 'success', t('dashboard.msg_resumed'));
            }
        } catch (error) {
            console.error('Pause/resume error:', error);
        }

        majBoutonPausePhotos();
    });

    // ── Bouton Arrêter ──
    document.getElementById('dash-stop-btn').addEventListener('click', async () => {
        if (!(await confirmer(t('dashboard.confirm_stop')))) {
            return;
        }

        const bouton = document.getElementById('dash-stop-btn');
        bouton.disabled = true;

        try {
            await terminerSession();
        } finally {
            bouton.disabled = false;
        }

        navigateTo('events');
        loadEvents();
    });

    // ── Exporter le journal (0.3.2) ──
    document.getElementById('dash-export-log-btn').addEventListener('click', exporterJournal);

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

    // Le débit moyen retombe quand plus rien ne part.
    setInterval(majDebit, 5000);
}

/**
 * Le bouton Pause ne concerne que les photos : sans surveillance photo, il
 * n'aurait aucun effet — il disparaît.
 */
function majBoutonPausePhotos() {
    const btn = document.getElementById('dash-pause-btn');

    btn.style.display = AppState.sessionId ? 'inline-flex' : 'none';

    if (!AppState.isPaused) {
        btn.innerHTML = `
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/>
            </svg>
            <span>${escapeHtml(t('dashboard.pause'))}</span>`;
    } else {
        btn.innerHTML = `
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <polygon points="5 3 19 12 5 21 5 3"/>
            </svg>
            <span>${escapeHtml(t('dashboard.resume'))}</span>`;
    }
}

// ═══════════════════════════════════════════════════════
// BANDEAU DE SESSION (0.3.1)
// ═══════════════════════════════════════════════════════
//
// Visible sur tous les écrans sauf la surveillance : il rappelle qu'une
// session tourne, ce qu'elle fait, et ramène à la surveillance d'un clic.

function initBandeauSession() {
    document.getElementById('session-banner-btn').addEventListener('click', () => {
        navigateTo('dashboard');
    });
}

function majBandeauSession() {
    const bandeau = document.getElementById('session-banner');

    if (!bandeau) {
        return;
    }

    const visible = sessionEnCours()
        && AppState.currentScreen !== 'dashboard'
        && AppState.currentScreen !== 'login';

    bandeau.style.display = visible ? 'flex' : 'none';

    if (!visible) {
        return;
    }

    document.getElementById('session-banner-title').textContent =
        t('nav.banner_title', { event: AppState.activeEvent.name });

    const parties = [];

    if (AppState.sessionId) {
        parties.push(t('nav.banner_photos', {
            sent: AppState.photoStats.sent,
            pending: AppState.photoStats.pending
        }));
    }

    if (videoSessionId) {
        parties.push(t('nav.banner_video_recording'));
    } else if (videoEnPause) {
        parties.push(t('nav.banner_video_paused'));
    }

    const attente = videoEnAttenteEvenement();

    if (attente > 0) {
        parties.push(t('nav.banner_video_sending', { count: attente }));
    }

    document.getElementById('session-banner-detail').textContent = parties.join(' · ');

    const point = document.getElementById('session-banner-dot');

    point.className = 'status-dot ' + (videoSessionId
        ? 'status-recording'
        : (videoEnPause || AppState.isPaused ? 'status-paused' : 'status-active'));
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
    //
    // 0.3.1 : plus de « Ko/s » par fichier — c'était la taille divisée par
    // le temps total, traitement serveur et attentes compris. On affiche la
    // part du réseau et celle du serveur (en-tête Server-Timing), mesurées
    // sur la seule tentative réussie.
    //
    // 0.3.2 : « réseau » est l'envoi du fichier, mesuré par l'agent (du
    // premier au dernier octet remis au réseau) ; « serveur » est la valeur
    // annoncée par le serveur (Server-Timing). Si les deux se recouvrent —
    // le chrono du serveur démarre avant la fin de la réception, il compte
    // alors le transfert —, la répartition n'a pas de sens : on affiche le
    // total et la valeur du serveur, sans rien en déduire.
    await listen('upload_success', (event) => {
        const { filename, size, duration_ms, server_ms, network_ms, duplicate, display } = event.payload;
        let message = Affichage.messageEnvoiPhoto(display, duration_ms, network_ms, server_ms, t);

        if (duplicate) {
            message += ' — ' + t('upload.server_had_it');
        }

        addLogEntry(timeNow(), filename, 'success', message);
        noterEnvoi(size || 0, duration_ms || 0);
    });

    // ── Photo absente de la galerie, que la mémoire locale croyait en
    //    ligne : le serveur fait foi, elle repart (0.3.3) ──
    await listen('upload_back', (event) => {
        const { filename } = event.payload;
        addLogEntry(timeNow(), filename, 'retry', t('upload.missing_on_server'));
    });

    // ── « Reprendre » a revérifié la galerie (0.3.4) ──
    await listen('gallery_rechecked', (event) => {
        const { checked, missing } = event.payload;

        addLogEntry(timeNow(), '—', missing > 0 ? 'retry' : 'success', missing > 0
            ? t('upload.gallery_recheck_missing', { count: missing })
            : t('upload.gallery_recheck_ok', { count: checked }));
    });

    // ── Photo déjà en ligne : non renvoyée (0.3.2) ──
    await listen('upload_already', (event) => {
        const { filename } = event.payload;
        addLogEntry(timeNow(), filename, 'success', t('upload.already_online'));
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

    // ── Serveur saturé (429 / 503) : l'agent ralentit de lui-même ──
    await listen('upload_throttled', (event) => {
        const { filename, code, delay_seconds, parallel } = event.payload;
        addLogEntry(timeNow(), filename, 'retry',
            t('upload.throttled', { code, delay: delay_seconds, parallel }));
    });

    // ── Stats mises à jour (après chaque upload) ──
    await listen('stats_updated', (event) => {
        const { sent, pending, failed } = event.payload;
        updateStats(sent, pending, failed);
    });

    // ── Tous les fichiers traités ──
    await listen('all_complete', (event) => {
        const { total_sent, total_failed, total_already } = event.payload;
        let msg = total_failed > 0
            ? t('complete.with_errors', { sent: total_sent, failed: total_failed })
            : t('complete.success', { count: total_sent });

        if (total_already > 0) {
            msg += ' ' + t('complete.already', { count: total_already });
        }

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
        AppState.photoReglages = null;
        majBoutonPausePhotos();
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
    ajouterLigneEcran(time, filename, status, message);

    // Recopie sur le disque, par paquets (voir journaliserSurDisque).
    journalEnAttente.push({ heure: String(time), fichier: String(filename), statut: status, message: String(message) });
}

/**
 * Affiche une ligne au journal de l'écran, sans la recopier sur le disque.
 */
function ajouterLigneEcran(time, filename, status, message) {
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

    // 0.3.2 : 5 000 lignes au lieu de 200. Une manche de 215 photos en
    // produit près de 700 : avec 200, son début était déjà perdu à la fin.
    while (log.children.length > JOURNAL_ECRAN_MAX) {
        log.removeChild(log.lastChild);
    }
}

// ─── Journal sur le disque (0.3.2) ───
//
// Chaque ligne affichée part aussi dans le journal de l'agent, sur le disque
// (dossier de données de l'agent, journaux\agent.log), avec le journal
// technique. Envoi groupé une fois par seconde : une rafale de photos ne
// coûte pas un appel par ligne.

const JOURNAL_ECRAN_MAX = 5000;
const journalEnAttente = [];

async function journaliserSurDisque() {
    if (journalEnAttente.length === 0) {
        return;
    }

    const lignes = journalEnAttente.splice(0, journalEnAttente.length);

    try {
        await invoke('journal_append', { lignes });
    } catch (e) {
        console.error('Journal error:', e);
    }
}

setInterval(journaliserSurDisque, 1000);

/**
 * « Exporter le journal » : un seul fichier, du plus ancien au plus récent,
 * à joindre à un compte rendu de test ou à un signalement.
 */
async function exporterJournal() {
    try {
        await journaliserSurDisque();

        const { save } = window.__TAURI__.dialog;
        const d = new Date();
        const deux = (n) => String(n).padStart(2, '0');
        const nom = `attimo-agent-journal-${d.getFullYear()}${deux(d.getMonth() + 1)}${deux(d.getDate())}-${deux(d.getHours())}${deux(d.getMinutes())}.log`;

        const destination = await save({
            title: t('journal.export_dialog'),
            defaultPath: nom,
            filters: [{ name: 'Journal', extensions: ['log', 'txt'] }],
        });

        if (!destination) {
            return;
        }

        await invoke('export_journal', { destination });

        addLogEntry(timeNow(), '—', 'success', t('journal.exported', { path: destination }));
        alert(t('journal.exported', { path: destination }));
    } catch (e) {
        addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error: e }));
        alert(t('dashboard.msg_error', { error: e }));
    }
}

// ─── Stats du dashboard ───

function updateStats(sent, pending, failed) {
    AppState.photoStats = { sent, pending, failed };

    document.getElementById('stat-sent').textContent = sent;
    document.getElementById('stat-pending').textContent = pending;
    document.getElementById('stat-failed').textContent = failed;

    const total = sent + pending + failed;
    const percent = total > 0 ? Math.round((sent / total) * 100) : 0;
    document.getElementById('progress-fill').style.width = `${percent}%`;
    document.getElementById('progress-text').textContent = `${sent} / ${total}  (${percent}%)`;

    // Afficher le bouton retry si des fichiers ont échoué
    document.getElementById('dash-retry-btn').style.display = failed > 0 ? 'inline-flex' : 'none';

    majBandeauSession();
}

// ─── Débit réel des envois (0.3.1) ───
//
// Octets effectivement envoyés sur la dernière minute, tous envois
// confondus : c'est le chiffre à comparer à un test de débit (fast.com),
// en mégabits par seconde comme lui. Il remplace le « Ko/s » d'un seul
// fichier, qui mêlait réseau, traitement serveur et attentes.
//
// 0.3.4 — Vidéo comprise : clips, versions légères et images d'analyse y
// entrent à chaque relevé de la file vidéo (releverOctetsVideo).

const FENETRE_DEBIT_MS = 60000;
const debitFenetre = [];

function noterEnvoi(octets, dureeMs) {
    const maintenant = Date.now();

    debitFenetre.push({ fin: maintenant, debut: maintenant - dureeMs, octets });
    majDebit();
}

function majDebit() {
    const maintenant = Date.now();

    while (debitFenetre.length > 0 && maintenant - debitFenetre[0].fin > FENETRE_DEBIT_MS) {
        debitFenetre.shift();
    }

    const zone = document.getElementById('stat-speed');

    if (debitFenetre.length === 0) {
        zone.textContent = '—';
        return;
    }

    // Durée observée : de l'envoi le plus ancien de la fenêtre à maintenant,
    // plafonnée à une minute.
    const debut = Math.max(maintenant - FENETRE_DEBIT_MS,
        Math.min(...debitFenetre.map(e => e.debut)));
    const secondes = Math.max(1, (maintenant - debut) / 1000);
    const octets = debitFenetre.reduce((somme, e) => somme + e.octets, 0);

    zone.textContent = ((octets * 8) / secondes / 1000000).toFixed(1);
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
//
// 0.3.1 — PAUSE. Une pause referme la captation en cours exactement comme
// un arrêt : le morceau ouvert est rattrapé, le clip en cours devient un
// clip final plus court, et tout part avec les autres. « Reprendre » ouvre
// une nouvelle captation avec les mêmes réglages, dans son propre
// sous-dossier : chaque reprise est une session de captation à part pour le
// serveur. Rien n'est filmé pendant la pause.
//
// 0.3.2 — NUMÉROTATION. Les clips ne repartent plus de 1 à chaque reprise :
// la numérotation continue sur toute l'épreuve (clip_0008 après clip_0007),
// le serveur ne reçoit plus deux « clip_0001 » dans le même événement.
// L'appariement HD ↔ version légère reste sûr : il se fait sur le couple
// (session de captation, numéro), et les deux variantes d'un clip portent
// toujours le même.
//
// 0.3.2 — RELANCE. Après « Arrêter la vidéo », « Relancer la captation »
// rouvre une captation dans la même session, avec les mêmes réglages, et la
// numérotation continue. L'arrêt demande confirmation.

let videoSessionId = null;       // captation qui tourne (FFmpeg actif)
let videoEnPause = false;
let videoDebutMs = 0;            // début de la captation courante
let videoCumulMs = 0;            // temps filmé avant la dernière reprise
let videoImages = 0;
let videoChronoTimer = null;

// Toutes les captations de la session (une par reprise), et ce qu'on sait
// de chacune : épreuve, découpage, clips filmés et assemblés.
let videoSessionsCourantes = [];
const videoCaptations = {};

// Réglages figés au premier démarrage : une reprise filme à l'identique,
// même si l'écran de réglages a été modifié entre-temps.
let videoConfigCaptation = null;
let videoCheckpointId = null;

// La session a comporté de la vidéo : le panneau reste affiché après
// l'arrêt, le temps que les derniers clips partent.
let videoActivite = false;

// Pause, reprise ou arrêt en cours (0.3.1) : les boutons le montrent et
// refusent un second clic.
let videoTransition = null;      // 'pausing' | 'resuming' | 'stopping' | null
let videoArretPromesse = null;
let videoPausePromesse = null;

// Vidage final : l'assemblage du dernier clip se termine APRÈS l'arrêt de
// la captation. Couper la file à ce moment laisse ce clip en attente
// indéfiniment. On la laisse donc tourner jusqu'à ce qu'elle soit vide.
let videoVidageEnCours = false;

// L'événement est figé au moment de l'arrêt : l'interface retourne à la
// liste des événements, et le photographe peut en ouvrir un autre pendant
// que les derniers fichiers montent encore.
let videoVidageEventId = null;

// Morceaux en cours de traitement. Tant que ce compteur n'est pas à zéro,
// un clip peut encore entrer en file : la déclarer vide serait prématuré.
let videoTraitementsEnCours = 0;

// ─── 0.3.2 ───

// Octets écrits par chaque captation de la session : chaque reprise a son
// propre dossier, donc son propre compteur ; l'écran les additionne.
let videoDisqueParCaptation = {};

// L'épreuve est sans identification : les images d'analyse ne sont ni
// extraites ni envoyées. Dit une seule fois au journal.
let videoAnalyseDesactivee = false;
let videoAnalyseAnnoncee = false;

// ─── 0.3.3 ───

// Dernier avis à montrer dans le bloc vidéo (fichiers retirés de la file).
let videoAvis = null;

// Épreuves dont le serveur a déjà été interrogé sur ses numéros de clip.
const videoServeurInterroge = new Set();

// ─── 0.3.4 ───

// Débit HD mesuré par l'agent pendant l'envoi réel des morceaux (Mb/s), ou
// null. Remplace les relevés de la 0.3.3, qui moyennaient aussi les temps
// morts.
let videoDebitHd = null;

// Ouverture de la session : le bloc HD ne fait le bilan que de ce qui est
// sorti de la file depuis (envoyés, introuvables).
let videoHdDepuisMs = Date.now();

// Dernier relevé des octets vidéo envoyés, pour la tuile « Mb/s envoyés ».
let videoOctetsReleve = null;

// Prochain numéro de clip, par épreuve. Tenu ici en plus de la file : le
// dernier clip d'une captation n'entre en file qu'après sa version légère,
// plusieurs secondes après l'arrêt — une reprise immédiate ne doit pas
// reprendre son numéro.
const videoNumeroParEvenement = {};

/**
 * Une captation est ouverte : elle tourne, ou elle est en pause.
 */
function captationOuverte() {
    return videoSessionId !== null || videoEnPause;
}

/**
 * Branche les écoutes du backend.
 *
 * Une seule fois au lancement : ces abonnements survivent aux captations
 * successives, et en créer un par session les empilerait.
 */
async function initVideo() {
    // L'arrêt demande confirmation (0.3.2) : un clic malheureux en course
    // ne doit pas couper la vidéo. Elle se relance ensuite d'un clic.
    document.getElementById('dash-video-stop-btn').addEventListener('click', async () => {
        if (!captationOuverte() || videoTransition) {
            return;
        }

        if (!(await confirmer(t('dashboard.video_stop_confirm')))) {
            return;
        }

        arreterCaptation();
    });

    document.getElementById('dash-video-restart-btn').addEventListener('click', () => {
        relancerCaptation();
    });

    document.getElementById('dash-video-pause-btn').addEventListener('click', () => {
        if (videoEnPause) {
            reprendreCaptation();
        } else {
            pauserCaptation();
        }
    });

    document.getElementById('dash-hd-btn').addEventListener('click', () => {
        activerEnvoiHd(!AppState.videoSendHd);
    });

    document.getElementById('dash-hd-retry-btn').addEventListener('click', async () => {
        try {
            const count = await invoke('retry_video_queue');
            addLogEntry(timeNow(), '—', 'retry', t('dashboard.msg_retry_queued', { count }));
            demarrerFileVideo();
        } catch (e) {
            addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error: e }));
        }
    });

    await listen('segment-ready', surMorceauPretSuivi);
    await listen('recording-final', surCaptationCoupee);
    await listen('disk-status', surEtatDisque);
}

/**
 * Lance une captation avec les réglages de l'écran.
 *
 * `reprise` : après une pause ou un arrêt, mêmes réglages que la première
 * captation. Le chrono ne repart jamais de zéro ici : il cumule toutes les
 * captations de la session, et seule une nouvelle session le remet à zéro
 * (preparerTableauDeBord).
 */
async function demarrerCaptation(reprise) {
    const evenement = AppState.activeEvent;

    if (!reprise) {
        const selectCp = document.getElementById('config-video-checkpoint');
        videoCheckpointId = selectCp.value ? parseInt(selectCp.value, 10) : null;
        videoConfigCaptation = construireConfigVideo();
    }

    // Numérotation (0.3.3) : le plus grand clip connu du serveur compte
    // aussi — autre poste, réinstallation, file purgée.
    await interrogerNumeroServeur(evenement.id);

    const sessionId = 'video_' + Date.now();
    const debut = Date.now();

    try {
        // L'autorisation d'envoi HD est un état du backend : il faut la
        // poser avant que le premier clip n'entre en file.
        await invoke('set_hd_upload', { allow: AppState.videoSendHd });

        const plan = await invoke('start_recording', {
            sessionId: sessionId,
            attimoTenantId: AppState.user ? AppState.user.id : 0,
            attimoEventId: evenement.id,
            attimoCheckpointId: videoCheckpointId,
            intervalSecs: VIDEO_FRAME_INTERVAL,
            config: videoConfigCaptation,
            firstClipIndex: videoNumeroParEvenement[evenement.id] || null
        });

        videoSessionId = sessionId;
        videoDebutMs = debut;
        videoEnPause = false;
        videoActivite = true;

        videoCaptations[sessionId] = {
            eventId: evenement.id,
            plan: plan,
            filmes: 0,
            assembles: 0,
        };

        videoSessionsCourantes.push(sessionId);

        if (!videoChronoTimer) {
            videoChronoTimer = setInterval(majChronoVideo, 1000);
        }

        addLogEntry(timeNow(), '—', 'success', reprise
            ? t('dashboard.video_resumed_log')
            : t('dashboard.video_started', { step: plan.step_secs }));

        if (plan.premier_numero > 1) {
            addLogEntry(timeNow(), '—', 'success',
                t('dashboard.video_numbering', { index: plan.premier_numero }));
        }

        // Une captation = un dossier video_… : après une pause ou une
        // relance, le journal dit où sont les nouveaux fichiers (0.3.3).
        if (plan.dossier) {
            addLogEntry(timeNow(), '—', 'success', t('dashboard.video_folder', { folder: plan.dossier }));
        }

        // L'épreuve accepte-t-elle les images d'analyse ? Demandé sans
        // retarder la captation ; dit une seule fois, en clair (0.3.2).
        sonderAnalyseVideo(evenement.id, sessionId);

        majPanneauVideo();
        majBandeauSession();

        return true;

    } catch (e) {
        addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error: e }));
        majPanneauVideo();
        return false;
    }
}

/**
 * Referme la captation qui tourne et rattrape son dernier morceau.
 *
 * Commun à la pause et à l'arrêt. Rend la main dès que FFmpeg a refermé
 * son fichier et que le dernier clip est assemblé ; la version légère de
 * ce clip — un réencodage, donc la partie la plus longue — se fait ensuite
 * en arrière-plan. C'est elle qui donnait l'impression d'un bouton « Arrêter
 * la vidéo » qui ne répondait pas : l'écran attendait sa fin sans rien
 * montrer.
 */
async function refermerCaptationCourante() {
    const sessionEnCours = videoSessionId;

    if (!sessionEnCours) {
        return;
    }

    let fin = null;

    try {
        // L'arrêt attend que FFmpeg ait refermé son fichier, et renvoie ce
        // qu'il a pu rattraper : le morceau resté ouvert, ses images, et le
        // ou les clips qu'il permet enfin d'assembler.
        fin = await invoke('stop_recording');
    } catch (e) {
        // ARRET_EN_COURS : la surveillance disque arrête déjà cette
        // captation, son résultat arrivera par l'événement recording-final.
        // « Aucune captation » : elle est déjà close. Rien à signaler.
        const texte = String(e);

        if (texte !== 'ARRET_EN_COURS' && !texte.includes('Aucune captation')) {
            addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error: e }));
        }
    }

    // La captation n'existe plus, quoi qu'il arrive ensuite : l'écran doit
    // le refléter tout de suite.
    if (videoSessionId === sessionEnCours) {
        videoCumulMs += Date.now() - videoDebutMs;
        videoSessionId = null;
    }

    if (fin) {
        // Compté : le vidage final ne doit pas conclure pendant que le
        // dernier clip se prépare.
        videoTraitementsEnCours++;

        traiterFinDeCaptation(fin, sessionEnCours).finally(() => {
            videoTraitementsEnCours--;
            majPanneauVideo();
        });
    }
}

/**
 * Pause : la captation s'interrompt, la session reste ouverte (0.3.1).
 *
 * Les clips déjà faits continuent de partir. Le clip en cours est refermé
 * comme à l'arrêt : il part, plus court, avec les autres.
 */
async function pauserCaptation() {
    if (!videoSessionId || videoTransition) {
        return;
    }

    videoTransition = 'pausing';
    majBoutonsVideo();

    videoPausePromesse = refermerCaptationCourante();

    try {
        await videoPausePromesse;
    } finally {
        videoPausePromesse = null;

        // Un arrêt demandé entre-temps garde la main sur l'affichage.
        if (videoTransition === 'pausing') {
            videoTransition = null;
        }
    }

    // Un arrêt a pu être demandé pendant la pause : il a la priorité.
    if (!videoArretPromesse) {
        videoEnPause = true;
        addLogEntry(timeNow(), '—', 'retry', t('dashboard.video_paused_log'));
    }

    majChronoVideo();
    majPanneauVideo();
    majBandeauSession();
}

/**
 * Reprise après une pause : mêmes réglages, nouvelle captation.
 */
async function reprendreCaptation() {
    if (!videoEnPause || videoTransition) {
        return;
    }

    videoTransition = 'resuming';
    majBoutonsVideo();

    try {
        await demarrerCaptation(true);
    } finally {
        if (videoTransition === 'resuming') {
            videoTransition = null;
        }
    }

    majPanneauVideo();
    majBandeauSession();
}

/**
 * Plus grand numéro de clip connu du serveur pour l'épreuve (0.3.3).
 *
 * Une fois par épreuve et par lancement : ensuite la numérotation ne fait
 * que monter localement. Sans réponse du serveur, rien ne change.
 */
async function interrogerNumeroServeur(eventId) {
    if (videoServeurInterroge.has(eventId)) {
        return;
    }

    videoServeurInterroge.add(eventId);

    try {
        const max = await invoke('video_server_clip_max', {
            token: AppState.token,
            eventId: eventId
        });

        if (max) {
            retenirNumeroClip(eventId, max + 1);
        }
    } catch (e) {
        console.error('Clip numbering error:', e);
    }
}

/**
 * « Relancer la captation » après un arrêt (0.3.2).
 *
 * Même session, mêmes réglages que la première captation ; le chrono
 * reprend son cumul et la numérotation des clips continue.
 */
async function relancerCaptation() {
    if (captationOuverte() || videoTransition || !videoConfigCaptation || !sessionEnCours()) {
        return;
    }

    videoTransition = 'resuming';
    majBoutonsVideo();

    let ok = false;

    try {
        ok = await demarrerCaptation(true);
    } finally {
        if (videoTransition === 'resuming') {
            videoTransition = null;
        }
    }

    if (ok) {
        addLogEntry(timeNow(), '—', 'success', t('dashboard.video_restarted_log'));
    }

    majPanneauVideo();
    majBandeauSession();
}

/**
 * Demande au serveur si l'épreuve analyse les images (0.3.2).
 *
 * Sur une galerie « Aucune », le serveur refuse les images d'analyse :
 * l'agent n'en extrait plus, n'en envoie plus, et le dit une fois.
 */
async function sonderAnalyseVideo(eventId, sessionId) {
    if (videoAnalyseDesactivee) {
        return;
    }

    try {
        const active = await invoke('video_analysis_probe', {
            token: AppState.token,
            eventId: eventId,
            sessionId: sessionId
        });

        if (active === false) {
            signalerAnalyseDesactivee();
        }
    } catch (e) {
        console.error('Analysis probe error:', e);
    }
}

function signalerAnalyseDesactivee() {
    videoAnalyseDesactivee = true;

    if (!videoAnalyseAnnoncee) {
        videoAnalyseAnnoncee = true;
        addLogEntry(timeNow(), '—', 'retry', t('dashboard.video_analysis_disabled'));
    }

    majPanneauVideo();
}

/**
 * Arrête la captation sans toucher à la surveillance des photos.
 *
 * Les fichiers déjà produits restent en file : couper la caméra ne doit
 * pas annuler ce qui attend d'être envoyé.
 *
 * 0.3.1 — Un seul arrêt à la fois : un second clic, ou « Terminer la
 * session » juste après, attend le même arrêt au lieu d'en lancer un
 * second. Le bouton affiche aussitôt « Arrêt en cours ».
 */
async function arreterCaptation() {
    if (videoArretPromesse) {
        return videoArretPromesse;
    }

    if (!captationOuverte() && !videoPausePromesse) {
        return;
    }

    videoArretPromesse = (async () => {
        videoTransition = 'stopping';
        majBoutonsVideo();

        // Une mise en pause en cours referme déjà la captation : on
        // l'attend plutôt que d'en demander une seconde fermeture.
        if (videoPausePromesse) {
            await videoPausePromesse.catch(() => {});
        }

        await refermerCaptationCourante();

        videoEnPause = false;

        if (videoChronoTimer) {
            clearInterval(videoChronoTimer);
            videoChronoTimer = null;
        }

        addLogEntry(timeNow(), '—', 'retry',
            t('dashboard.video_stopped', { clips: videoTotal('assembles') }));
    })();

    try {
        await videoArretPromesse;
    } finally {
        videoArretPromesse = null;
        videoTransition = null;
        majChronoVideo();
        majPanneauVideo();
        majBandeauSession();
    }
}

/**
 * Enveloppe de surMorceauPret qui compte les traitements en cours.
 *
 * Le comptage est isolé ici pour ne pas parsemer la fonction de traitement
 * de compteurs : elle a plusieurs sorties, et en oublier une bloquerait le
 * vidage final pour toujours.
 */
async function surMorceauPretSuivi(evt) {
    videoTraitementsEnCours++;

    try {
        await surMorceauPret(evt);
    } finally {
        videoTraitementsEnCours--;
    }
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
    // en file sous une session qui n'existe plus — ou, depuis la pause, sous
    // la captation suivante (0.3.1).
    //
    // Le fichier n'est pas perdu pour autant — il reste sur le disque, et
    // le morceau incomplet n'avait de toute façon pas sa place dans un clip.
    if (!videoSessionId || morceau.session_id !== videoSessionId) {
        return;
    }

    // L'identifiant est figé maintenant. Le traitement d'un morceau dure
    // plusieurs secondes — extraction, assemblage, réencodage — et l'arrêt
    // peut tomber pendant. Sans cette copie, la fin du traitement
    // travaillerait sous une session déjà refermée.
    const sessionEnCours = videoSessionId;

    compterClipsFilmes(sessionEnCours, morceau.index);

    // Images d'analyse — extraites du MORCEAU, jamais du clip.
    //
    // Les clips se chevauchent : extraire depuis eux traiterait deux fois
    // les mêmes instants, et ferait payer deux fois la reconnaissance pour
    // un résultat identique.
    //
    // Galerie sans identification (0.3.2) : aucune image, ni extraite ni
    // envoyée — le serveur les refuserait.
    if (!videoAnalyseDesactivee) {
        try {
            const res = await invoke('extract_frames', {
                segmentIndex: morceau.index,
                intervalSecs: VIDEO_FRAME_INTERVAL
            });

            await mettreImagesEnFile(res.frames, sessionEnCours);
        } catch (e) {
            addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error: e }));
        }
    }

    // Chaque morceau peut compléter un clip. On demande à l'assembleur.
    try {
        const clip = await invoke('on_segment_ready', { segmentIndex: morceau.index });

        if (!clip) {
            return;
        }

        await mettreClipEnFile(clip, sessionEnCours);

    } catch (e) {
        addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error: e }));
    }
}

/**
 * Clips entièrement filmés, d'après le dernier morceau clos (0.3.1).
 *
 * Le clip n couvre les morceaux (n-1)·pas à (n-1)·pas + parClip - 1 : il
 * est filmé dès que son dernier morceau est clos, avant même d'être
 * assemblé.
 */
function compterClipsFilmes(sessionId, indexMorceau) {
    const captation = videoCaptations[sessionId];

    if (!captation || !captation.plan) {
        return;
    }

    const parClip = captation.plan.segments_per_clip;
    const pas = captation.plan.segments_step;
    const clos = indexMorceau + 1;

    const filmes = clos >= parClip ? Math.floor((clos - parClip) / pas) + 1 : 0;

    captation.filmes = Math.max(captation.filmes, filmes);
    majPanneauVideo();
}

/**
 * Somme d'un compteur sur toutes les captations de la session.
 */
function videoTotal(champ) {
    return videoSessionsCourantes.reduce((somme, id) => {
        const captation = videoCaptations[id];
        return somme + (captation ? captation[champ] : 0);
    }, 0);
}

/**
 * Fin de captation : ce que l'arrêt a rattrapé entre en file.
 *
 * Le dernier morceau n'est jamais annoncé par FFmpeg — son annonce serait
 * déclenchée par l'ouverture du morceau suivant, qui n'existe pas. C'est
 * donc l'arrêt lui-même qui le traite, et qui renvoie le résultat ici :
 * ses images, et le ou les clips qu'il permet enfin d'assembler.
 */
async function traiterFinDeCaptation(fin, sessionEnCours) {
    if (fin.avertissement) {
        addLogEntry(timeNow(), '—', 'failed',
            t('dashboard.msg_error', { error: fin.avertissement }));
    }

    // La captation suivante — reprise, relance — continue la numérotation.
    retenirNumeroClip(evenementDeCaptation(sessionEnCours), fin.prochain_numero);

    // 0.3.4 — Comptés tout de suite, avant la première attente : l'arrêt
    // écrit « Captation arrêtée — N clip(s) produit(s) » dès que cette
    // fonction lui rend la main. Comptés plus bas, après la mise en file des
    // images, les clips refermés à l'arrêt manquaient au message (4 pour 5).
    for (const clip of fin.clips) {
        compterClipAssemble(clip, sessionEnCours);
    }

    try {
        if (!videoAnalyseDesactivee) {
            await mettreImagesEnFile(fin.frames, sessionEnCours);
        }

        for (const clip of fin.clips) {
            await mettreClipEnFile(clip, sessionEnCours, true);
        }
    } catch (e) {
        addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error: e }));
    }
}

/**
 * Le disque plein a coupé la captation depuis le backend.
 *
 * Personne n'attend de valeur de retour dans ce cas : le résultat arrive
 * par événement. Le compteur des traitements est indispensable ici — sans
 * lui, le vidage final déclarerait la file vide pendant que le dernier clip
 * s'assemble encore.
 */
async function surCaptationCoupee(evt) {
    videoTraitementsEnCours++;

    try {
        await traiterFinDeCaptation(evt.payload, evt.payload.session_id);
    } finally {
        videoTraitementsEnCours--;
    }
}

/**
 * Épreuve d'une captation : celle de son démarrage, pas celle qu'on
 * regarde à l'écran — on peut désormais en ouvrir une autre pendant que
 * les derniers clips se préparent.
 */
function evenementDeCaptation(sessionId) {
    const captation = videoCaptations[sessionId];

    if (captation) {
        return captation.eventId;
    }

    if (AppState.activeEvent) {
        return AppState.activeEvent.id;
    }

    return videoVidageEventId;
}

/**
 * Met en file les images d'analyse d'un morceau.
 *
 * Mise en file plutôt qu'envoi direct : c'est ce qui permet de couper
 * l'envoi vidéo sans rien perdre, et de le reprendre plus tard — y compris
 * après avoir fermé l'agent.
 */
async function mettreImagesEnFile(images, sessionEnCours) {
    const eventId = evenementDeCaptation(sessionEnCours);

    for (const image of images) {
        await invoke('queue_video_file', {
            sessionId: sessionEnCours,
            eventId: eventId,
            kind: 'frames',
            filePath: image.path,
            clipIndex: null,
            startedAt: null,
            endedAt: null,
            instantAt: image.instant_at
        });
    }

    videoImages += images.length;
    majPanneauVideo();
}

/**
 * Un clip de plus est assemblé : compteurs et numérotation.
 */
function compterClipAssemble(clip, sessionEnCours) {
    const captation = videoCaptations[sessionEnCours];

    if (captation) {
        captation.assembles++;
        captation.filmes = Math.max(captation.filmes, captation.assembles);
    }

    retenirNumeroClip(evenementDeCaptation(sessionEnCours), clip.index + 1);

    majPanneauVideo();
}

/**
 * Version légère, puis mise en file des deux variantes d'un clip.
 *
 * Sortie de surMorceauPret pour que l'arrêt de captation emprunte
 * exactement le même chemin : un clip de fin ne doit pas être traité
 * autrement qu'un clip de course.
 *
 * `dejaCompte` : l'arrêt a compté ses clips d'avance (0.3.4).
 */
async function mettreClipEnFile(clip, sessionEnCours, dejaCompte) {
    const eventId = evenementDeCaptation(sessionEnCours);

    if (!dejaCompte) {
        compterClipAssemble(clip, sessionEnCours);
    }

    addLogEntry(timeNow(), clip.filename, 'success',
        t('dashboard.video_clip_ready', { index: clip.index }));

    // Version légère : c'est elle que le coureur regarde pendant la
    // course, le fichier lourd pouvant monter le soir.
    const proxyPath = await invoke('generate_proxy', {
        clipPath: clip.path,
        height: 540,
        bitrateMbps: 2
    });

    // Horodatages du clip : ceux de l'agent (0.3.4), datés sur le même
    // départ que les images d'analyse, fin mesurée à la milliseconde pour
    // les clips de fin. Avant, l'interface les calculait depuis l'heure du
    // clic, prise un peu plus tôt que le départ des images : la fin des
    // clips tombait avant les dernières images, qui restaient sans clip.
    const debutIso = clip.started_at;
    const finIso = clip.ended_at;

    // Les deux variantes entrent en file. L'ordre d'envoi réel dépendra
    // de la priorité et de l'autorisation HD, pas de l'ordre d'ajout.
    await invoke('queue_video_file', {
        sessionId: sessionEnCours,
        eventId: eventId,
        kind: 'clip_proxy',
        filePath: proxyPath,
        clipIndex: clip.index,
        startedAt: debutIso,
        endedAt: finIso,
        instantAt: null
    });

    await invoke('queue_video_file', {
        sessionId: sessionEnCours,
        eventId: eventId,
        kind: 'clip_hd',
        filePath: clip.path,
        clipIndex: clip.index,
        startedAt: debutIso,
        endedAt: finIso,
        instantAt: null
    });

    rafraichirFileVideo();
}

/**
 * Retient le prochain numéro de clip libre pour une épreuve (0.3.2).
 */
function retenirNumeroClip(eventId, prochain) {
    if (!eventId || !prochain) {
        return;
    }

    videoNumeroParEvenement[eventId] = Math.max(videoNumeroParEvenement[eventId] || 1, prochain);
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

    // Cumul de toutes les captations de la session (0.3.2) : chaque reprise
    // écrit dans son propre dossier, et la surveillance repartait de zéro.
    if (etat.session_id) {
        videoDisqueParCaptation[etat.session_id] = etat.written_bytes;
    }

    const cumul = videoSessionsCourantes.reduce(
        (somme, id) => somme + (videoDisqueParCaptation[id] || 0), 0);

    const ecrits = (Math.max(cumul, etat.written_bytes) / 1073741824).toFixed(1);
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

    // Le backend a coupé la captation : l'écran doit suivre. Avant 0.3.1, le
    // panneau restait affiché avec un bouton « Arrêter la vidéo » qui ne
    // faisait plus rien — la captation n'existait déjà plus.
    if (etat.level === 'stopped') {
        addLogEntry(timeNow(), '—', 'failed', t('dashboard.video_disk_full'));

        if (videoSessionId) {
            videoCumulMs += Date.now() - videoDebutMs;
        }

        videoSessionId = null;
        videoEnPause = false;

        if (videoChronoTimer) {
            clearInterval(videoChronoTimer);
            videoChronoTimer = null;
        }

        majPanneauVideo();
        majBandeauSession();
    }
}

function majChronoVideo() {
    const courant = videoSessionId ? Date.now() - videoDebutMs : 0;
    const secondes = Math.floor((videoCumulMs + courant) / 1000);

    const h = String(Math.floor(secondes / 3600)).padStart(2, '0');
    const m = String(Math.floor((secondes % 3600) / 60)).padStart(2, '0');
    const s = String(secondes % 60).padStart(2, '0');

    document.getElementById('dash-video-elapsed').textContent = `${h}:${m}:${s}`;
}

// ─── Panneau vidéo du tableau de bord (0.3.1) ───

// Derniers décomptes de la file : toute l'épreuve (bloc HD, bandeau) et
// captations de la session en cours (compteurs de clips).
let videoStatsEvenement = null;
let videoStatsSession = null;

/**
 * Clips et images qui attendent encore de partir, pour toute l'épreuve.
 * Les clips HD retenus ne comptent que si leur envoi est autorisé.
 */
function videoEnAttenteEvenement() {
    const s = videoStatsEvenement;

    if (!s) {
        return 0;
    }

    const hd = AppState.videoSendHd ? s.hd_pending + s.hd_sending : 0;

    return s.frames_pending + s.frames_sending + s.proxy_pending + s.proxy_sending + hd;
}

function majPanneauVideo() {
    const panneau = document.getElementById('dash-video-panel');
    const evt = videoStatsEvenement;

    const aDesClips = evt !== null && (evt.hd_pending + evt.hd_sending + evt.hd_sent + evt.hd_failed
        + evt.proxy_pending + evt.proxy_sending + evt.proxy_sent) > 0;

    const visible = sessionEnCours() && (videoActivite || AppState.envoiSeul || aDesClips);

    panneau.style.display = visible ? 'block' : 'none';

    if (!visible) {
        return;
    }

    // ── Compteurs de la session (B4) ──
    const ses = videoStatsSession;

    //
    // 0.3.2 — « envoyés » compte les clips que le serveur a confirmés (sa
    // réponse, ou le recoupement de reprise), un clip compté une fois même
    // s'il a ses deux variantes. « En attente » : versions légères pas
    // encore reçues, en file ou en cours d'envoi ; les abandons sont dits à
    // part au lieu de rester comptés comme en attente.
    document.getElementById('dash-video-filmed').textContent = videoTotal('filmes');
    document.getElementById('dash-video-clips').textContent = videoTotal('assembles');
    document.getElementById('dash-video-sent').textContent = ses ? ses.clips_online : 0;

    // Le compteur est celui de la session. Si l'épreuve compte d'autres
    // clips en ligne (session précédente, autre sortie), on le dit : sans
    // cela, « 7 envoyés » se lisait comme faux face aux 19 de la galerie.
    const enLigneEpreuve = evt ? evt.clips_online : 0;
    const enLigneSession = ses ? ses.clips_online : 0;

    document.getElementById('dash-video-sent-label').textContent = enLigneEpreuve > enLigneSession
        ? t('dashboard.video_sent') + ' — ' + t('dashboard.video_online_event', { total: enLigneEpreuve })
        : t('dashboard.video_sent');
    document.getElementById('dash-video-queue').textContent =
        ses ? ses.proxy_pending + ses.proxy_sending : 0;
    document.getElementById('dash-video-waiting-label').textContent = ses && ses.proxy_failed > 0
        ? t('dashboard.video_waiting_failed', { failed: ses.proxy_failed })
        : t('dashboard.video_waiting');
    document.getElementById('dash-video-frames').textContent = videoImages;
    document.getElementById('dash-video-frames-label').textContent = videoAnalyseDesactivee
        ? t('dashboard.video_frames_disabled')
        : t('dashboard.video_frames_detail', { sent: ses ? ses.frames_sent : 0 });

    // Sans captation dans cette session, seuls l'envoi et le bloc HD ont
    // un sens.
    document.getElementById('dash-video-stats').style.display = videoActivite ? '' : 'none';
    document.getElementById('dash-video-disk').style.display = captationOuverte() ? '' : 'none';

    // Avis (0.3.3) : fichiers retirés de la file, rappelés ici.
    const avis = document.getElementById('dash-video-notice');
    avis.textContent = videoAvis || '';
    avis.style.display = videoAvis ? '' : 'none';

    // HD envoyée automatiquement (0.3.3) : à savoir sur un forfait 4G.
    document.getElementById('dash-hd-auto').style.display =
        AppState.videoSendHd && (videoActivite || captationOuverte()) ? '' : 'none';

    majBlocHd(evt);
    majBoutonsVideo();
}

/**
 * État et boutons de la captation.
 */
function majBoutonsVideo() {
    const titre = document.getElementById('dash-video-title');
    const point = document.getElementById('dash-video-dot');
    const chrono = document.getElementById('dash-video-elapsed');
    const pause = document.getElementById('dash-video-pause-btn');
    const stop = document.getElementById('dash-video-stop-btn');
    const relance = document.getElementById('dash-video-restart-btn');

    const ouverte = captationOuverte() || videoTransition !== null;

    pause.style.display = ouverte ? 'inline-flex' : 'none';
    stop.style.display = ouverte ? 'inline-flex' : 'none';
    chrono.style.display = videoActivite ? '' : 'none';

    // « Relancer la captation » (0.3.2) : après un arrêt, tant que la
    // session est ouverte et qu'une captation a déjà eu lieu.
    relance.style.display = (!ouverte && videoActivite && videoConfigCaptation
        && sessionEnCours() && !AppState.envoiSeul) ? 'inline-flex' : 'none';

    pause.disabled = videoTransition !== null;
    stop.disabled = videoTransition !== null;
    relance.disabled = videoTransition !== null;

    let titreTexte;
    let pointClasse;
    let pauseTexte = videoEnPause ? t('dashboard.video_resume') : t('dashboard.video_pause');
    let stopTexte = t('dashboard.video_stop');

    if (videoTransition === 'stopping') {
        titreTexte = t('dashboard.video_recording');
        pointClasse = 'status-paused';
        stopTexte = t('dashboard.video_stopping');
    } else if (videoTransition === 'pausing') {
        titreTexte = t('dashboard.video_recording');
        pointClasse = 'status-paused';
        pauseTexte = t('dashboard.video_pausing');
    } else if (videoTransition === 'resuming') {
        titreTexte = t('dashboard.video_paused');
        pointClasse = 'status-paused';
        pauseTexte = t('dashboard.video_resuming');
    } else if (videoSessionId) {
        titreTexte = t('dashboard.video_recording');
        pointClasse = 'status-recording';
    } else if (videoEnPause) {
        titreTexte = t('dashboard.video_paused');
        pointClasse = 'status-paused';
    } else if (videoActivite) {
        titreTexte = t('dashboard.video_stopped_title');
        pointClasse = 'status-idle';
    } else {
        titreTexte = t('dashboard.video_sending_only');
        pointClasse = 'status-idle';
    }

    titre.textContent = titreTexte;
    point.className = 'status-dot ' + pointClasse;

    // Le libellé change dès le clic : le photographe voit que sa demande
    // est prise en compte, même si l'arrêt dure quelques secondes.
    pause.querySelector('span').textContent = pauseTexte;
    stop.querySelector('span').textContent = stopTexte;

    pause.querySelector('.icon-pause').style.display = videoEnPause ? 'none' : '';
    pause.querySelector('.icon-play').style.display = videoEnPause ? '' : 'none';
    stop.querySelector('.spinner').style.display = videoTransition === 'stopping' ? '' : 'none';
    stop.querySelector('.icon-stop').style.display = videoTransition === 'stopping' ? 'none' : '';
}

/**
 * Bloc « Vidéo HD » (B5) : combien de clips pleine qualité restent, et un
 * bouton pour les envoyer maintenant.
 */
function majBlocHd(evt) {
    const statut = document.getElementById('dash-hd-status');
    const progression = document.getElementById('dash-hd-progress');
    const jauge = document.getElementById('dash-hd-fill');
    const bouton = document.getElementById('dash-hd-btn');
    const relance = document.getElementById('dash-hd-retry-btn');

    if (!evt) {
        statut.textContent = '';
        progression.textContent = '';
        jauge.style.width = '0%';
        bouton.style.display = 'none';
        relance.style.display = 'none';
        return;
    }

    // 0.3.3 : libellés dans affichage.js (testés), avec les clips
    // introuvables sur le disque, le débit et le temps restant.
    //
    // 0.3.4 : débit mesuré par l'agent pendant l'envoi réel, en envoi
    // manuel comme en HD automatique.
    const mbps = AppState.videoSendHd ? videoDebitHd : null;
    const bloc = Affichage.blocHd(evt, AppState.videoSendHd, mbps, t);
    const restant = bloc.restant;

    statut.textContent = bloc.statut;
    progression.textContent = bloc.progression;
    jauge.style.width = bloc.jauge + '%';

    bouton.style.display = restant > 0 ? 'inline-flex' : 'none';
    bouton.textContent = AppState.videoSendHd ? t('hd.suspend') : t('hd.send_now');
    bouton.className = 'btn ' + (AppState.videoSendHd ? 'btn-secondary' : 'btn-primary');

    relance.style.display = bloc.echecs > 0 ? 'inline-flex' : 'none';
}

/**
 * Autorise ou suspend l'envoi des clips HD.
 *
 * Même réglage que « Envoyer la pleine qualité pendant la course » dans
 * les réglages : les deux restent d'accord.
 */
async function activerEnvoiHd(activer) {
    // Un clip HD supprimé du disque sort de la file avant l'envoi (0.3.2).
    if (activer && AppState.activeEvent) {
        await purgerFichiersVideoAbsents(AppState.activeEvent.id);
    }

    try {
        await invoke('set_hd_upload', { allow: activer });
    } catch (e) {
        addLogEntry(timeNow(), '—', 'failed', t('dashboard.msg_error', { error: e }));
        return;
    }

    AppState.videoSendHd = activer;
    document.getElementById('config-video-hd').checked = activer;

    addLogEntry(timeNow(), '—', activer ? 'success' : 'retry',
        activer ? t('hd.started_log') : t('hd.suspended_log'));

    if (activer) {
        demarrerFileVideo();
    }

    majPanneauVideo();
    rafraichirFileVideo();
}

// ═══════════════════════════════════════════════════════════════════════
// FILE D'ENVOI VIDÉO
// ═══════════════════════════════════════════════════════════════════════
//
// 0.3.1 — Deux ouvriers, qui attendent chacun la fin de leur envoi avant
// d'en commencer un autre. Avant, un minuteur lançait un envoi chaque
// seconde sans attendre le précédent : tant que la file était pleine, les
// envois s'empilaient — des dizaines de clips à la fois, qui prenaient la
// liaison aux photos. Le backend refuse de toute façon un troisième envoi
// simultané, et fait passer les photos d'abord.

// 0.3.2 — Une voie par ouvrier : l'un sert d'abord les clips, l'autre
// d'abord les images d'analyse ; une voie sans travail prend celui de
// l'autre. Avant, les images passaient toujours devant : pendant la
// captation leur file ne se vidait jamais, et aucun clip ne partait avant
// « Arrêter la vidéo ».
const VIDEO_VOIES = ['clips', 'images'];

let videoFileActive = false;
const videoVoiesActives = new Set();
let videoStatsTimer = null;
let videoRafraichissementEnCours = false;
let videoRafraichissements = 0;
let videoDerniereSaturation = 0;

function demarrerFileVideo() {
    videoFileActive = true;

    for (const voie of VIDEO_VOIES) {
        if (videoVoiesActives.has(voie)) {
            continue;
        }

        videoVoiesActives.add(voie);

        ouvrierFileVideo(voie).finally(() => {
            videoVoiesActives.delete(voie);
        });
    }

    if (!videoStatsTimer) {
        videoStatsTimer = setInterval(rafraichirFileVideo, 2000);
    }

    rafraichirFileVideo();
}

function arreterFileVideo() {
    videoFileActive = false;

    if (videoStatsTimer) {
        clearInterval(videoStatsTimer);
        videoStatsTimer = null;
    }

    videoVidageEnCours = false;
    videoVidageEventId = null;
}

/**
 * Bascule la file en vidage final.
 *
 * Appelé à l'arrêt de la session : la file n'est pas coupée, elle continue
 * d'écouler ce qui reste puis s'arrête seule. Si l'agent est fermé entre
 * temps, rien n'est perdu — la file est persistante et reprendra à la
 * prochaine ouverture de cet événement.
 */
function demarrerVidageFinal() {
    if (AppState.activeEvent) {
        videoVidageEventId = AppState.activeEvent.id;
    }

    videoVidageEnCours = true;
    demarrerFileVideo();
}

/**
 * Épreuves dont la file est servie : celle de la session en cours d'abord,
 * puis celle d'une session close qui finit de partir.
 */
function evenementsFileVideo() {
    const ids = [];

    if (AppState.activeEvent) {
        ids.push(AppState.activeEvent.id);
    }

    if (videoVidageEnCours && videoVidageEventId && !ids.includes(videoVidageEventId)) {
        ids.push(videoVidageEventId);
    }

    return ids;
}

async function ouvrierFileVideo(voie) {
    while (videoFileActive) {
        const attente = await traiterUnEnvoiVideo(voie);

        if (attente > 0) {
            await dormir(attente);
        }
    }
}

/**
 * Un envoi : un clip, ou un lot de dix images d'analyse.
 *
 * Renvoie le temps à attendre avant le suivant, en millisecondes.
 */
async function traiterUnEnvoiVideo(voie) {
    const ids = evenementsFileVideo();

    if (ids.length === 0 || !AppState.token) {
        return 2000;
    }

    for (const eventId of ids) {
        let resultat;

        try {
            resultat = await invoke('process_video_queue', {
                token: AppState.token,
                eventId: eventId,
                checkpointId: null,
                voie: voie
            });
        } catch (e) {
            if (e === 'SESSION_EXPIRED') {
                arreterFileVideo();
                return 0;
            }

            // Serveur injoignable : on réessaie un peu plus tard.
            return 3000;
        }

        if (!resultat) {
            continue;
        }

        if (resultat.busy) {
            return 1000;
        }

        if (resultat.throttled) {
            // Une ligne de journal suffit, pas une par ouvrier.
            if (Date.now() - videoDerniereSaturation > 30000) {
                videoDerniereSaturation = Date.now();
                addLogEntry(timeNow(), '—', 'retry',
                    t('dashboard.video_throttled', { delay: resultat.throttled }));
            }

            return resultat.throttled * 1000;
        }

        if (resultat.detail) {
            journaliserEnvoiVideo(resultat.detail);
            rafraichirFileVideo();
            return 0;
        }

        // Fichiers disparus du disque : sortis de la file (0.3.2).
        if (resultat.missing) {
            journaliserAbsents(resultat.missing);
            rafraichirFileVideo();
            return 0;
        }

        // Galerie sans identification (0.3.2) : dit une fois, puis plus
        // aucune image ne part.
        if (resultat.analysis_disabled) {
            signalerAnalyseDesactivee();
            rafraichirFileVideo();
            return 0;
        }

        if (resultat.error) {
            if (resultat.definitive) {
                addLogEntry(timeNow(), '—', 'failed', libelleEchecVideo('dashboard.video_send_refused', resultat));
            } else if (resultat.abandoned) {
                addLogEntry(timeNow(), '—', 'failed', libelleEchecVideo('dashboard.video_send_failed', resultat));
            }

            rafraichirFileVideo();
            return 2000;
        }

        return 0;
    }

    return 2000;
}

/**
 * Ligne de journal d'un envoi vidéo abandonné : le clip concerné, s'il y en
 * a un, et le message du serveur.
 */
function libelleEchecVideo(cle, resultat) {
    const quoi = resultat.clip_index
        ? t('dashboard.video_clip_label', { index: resultat.clip_index })
        : t('dashboard.video_frames_label');

    return t(cle, { error: `${quoi} — ${resultat.error}` });
}

/**
 * Fichiers vidéo sortis de la file parce qu'ils ont disparu du disque.
 */
function journaliserAbsents(absents) {
    if (!absents || absents.length === 0) {
        return;
    }

    const clips = [...new Set(absents
        .filter(a => a.clip_index)
        .map(a => a.clip_index))]
        .sort((a, b) => a - b);

    const message = clips.length > 0
        ? t('dashboard.video_missing_clips', { count: absents.length, clips: clips.join(', ') })
        : t('dashboard.video_missing', { count: absents.length });

    const heure = timeNow();

    addLogEntry(heure, '—', 'retry', message);

    // Aussi dans le bloc vidéo (0.3.3) : au milieu du journal, la ligne se
    // perdait.
    videoAvis = message;
    majPanneauVideo();

    // Purge faite à l'ouverture de l'épreuve, hors session : la ligne est
    // reprise au journal de la session qui démarre, qui repart à blanc.
    if (!sessionEnCours()) {
        journalAvantSession.push({ heure, message, ms: Date.now() });
    }
}

// Lignes à reprendre au journal de la prochaine session (0.3.2).
const journalAvantSession = [];

/**
 * Sort de la file les fichiers vidéo disparus du disque (0.3.2), avec une
 * ligne au journal. Sans erreur au clic sur « Envoyer les HD ».
 */
async function purgerFichiersVideoAbsents(eventId) {
    if (!eventId) {
        return;
    }

    try {
        journaliserAbsents(await invoke('purge_missing_video_files', { eventId }));
    } catch (e) {
        console.error('Purge error:', e);
    }
}

function journaliserEnvoiVideo(detail) {
    if (detail.kind === 'clip') {
        const cle = detail.variant === 'hd'
            ? 'dashboard.video_clip_hd_sent'
            : 'dashboard.video_clip_sent';

        addLogEntry(timeNow(), '—', 'success', t(cle, { index: detail.clip.index }));
    }

    if (detail.totals && detail.totals.bibs > 0) {
        addLogEntry(timeNow(), '—', 'success',
            t('dashboard.video_bibs_read', { count: detail.totals.bibs }));
    }
}

/**
 * Octets vidéo envoyés depuis le dernier relevé (0.3.4).
 *
 * La tuile « Mb/s envoyés (1 min) » ne comptait que les photos : elle
 * restait à « — » pendant tout envoi vidéo. Chaque relevé y ajoute les
 * octets vidéo partis depuis le précédent — clips, versions légères, images
 * d'analyse. Le débit HD, lui, vient tel quel de l'agent.
 */
async function releverOctetsVideo() {
    let octets;

    try {
        octets = await invoke('video_bytes_sent');
    } catch (e) {
        return;
    }

    const maintenant = Date.now();

    videoDebitHd = typeof octets.hd_mbps === 'number' ? octets.hd_mbps : null;

    if (videoOctetsReleve && octets.total > videoOctetsReleve.total) {
        debitFenetre.push({
            debut: videoOctetsReleve.t,
            fin: maintenant,
            octets: octets.total - videoOctetsReleve.total
        });

        majDebit();
    }

    videoOctetsReleve = { t: maintenant, total: octets.total };
}

/**
 * Relit la file : compteurs, bloc HD, fin du vidage final.
 */
async function rafraichirFileVideo() {
    if (videoRafraichissementEnCours) {
        return;
    }

    videoRafraichissementEnCours = true;

    try {
        // Octets vidéo envoyés (0.3.4) : la tuile « Mb/s envoyés » les compte
        // avec les photos, et le bloc HD en tire son débit. Relevés avant
        // tout le reste : le vidage final envoie encore sans épreuve ouverte.
        await releverOctetsVideo();

        // Fin du vidage final. Les deux conditions comptent : une file vide
        // pendant qu'un morceau s'assemble encore serait un faux signal, et
        // le clip produit une seconde plus tard resterait en attente. Les
        // clips HD retenus ne le retiennent pas : ils attendent le bouton
        // « Envoyer les HD maintenant ».
        if (videoVidageEnCours && videoVidageEventId
            && !(AppState.activeEvent && AppState.activeEvent.id === videoVidageEventId)) {
            try {
                const s = await invoke('video_queue_stats', {
                    eventId: videoVidageEventId,
                    sessions: null
                });

                const hd = AppState.videoSendHd ? s.hd_pending + s.hd_sending : 0;
                const reste = s.frames_pending + s.frames_sending
                    + s.proxy_pending + s.proxy_sending + hd;

                if (reste === 0 && videoTraitementsEnCours === 0) {
                    videoVidageEnCours = false;
                    videoVidageEventId = null;
                }
            } catch (e) {
                // Lecture ratée : on garde le vidage, par prudence.
            }
        }

        if (!AppState.activeEvent) {
            if (!videoVidageEnCours) {
                arreterFileVideo();
            }

            return;
        }

        const eventId = AppState.activeEvent.id;

        // Toutes les 30 s environ : un fichier supprimé du disque pendant
        // la session sort de la file (0.3.2).
        videoRafraichissements++;

        if (videoRafraichissements % 15 === 0) {
            await purgerFichiersVideoAbsents(eventId);
        }

        try {
            videoStatsEvenement = await invoke('video_queue_stats', {
                eventId: eventId,
                sessions: null,
                hdSinceMs: videoHdDepuisMs
            });

            videoStatsSession = videoSessionsCourantes.length > 0
                ? await invoke('video_queue_stats', {
                    eventId: eventId,
                    sessions: videoSessionsCourantes
                })
                : null;
        } catch (e) {
            // Sans importance : l'affichage se rafraîchira au prochain passage.
        }

        majPanneauVideo();
        majBandeauSession();

    } finally {
        videoRafraichissementEnCours = false;
    }
}
