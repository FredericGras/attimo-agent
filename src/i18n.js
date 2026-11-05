// ═══════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Module i18n (internationalisation)
// Détection automatique de la langue OS + traductions
// ═══════════════════════════════════════════════════════
//
// 28 langues supportées — identiques à Attimo Gallery.
// Détecte la langue de l'OS du photographe au lancement,
// charge le fichier JSON correspondant, et traduit l'UI.
//
// Utilisation :
//   - HTML statique : <span data-i18n="login.submit">Se connecter</span>
//   - JS dynamique  : t('upload.success', { duration: '2.3', speed: '450' })
//

// ─── Langues supportées (28) ───

const SUPPORTED_LOCALES = [
    'fr', 'en', 'de', 'es', 'it', 'pt', 'pt_BR', 'nl',
    'da', 'sv', 'no', 'fi',
    'pl', 'cs', 'sk', 'hu', 'ro', 'bg', 'hr', 'sl',
    'et', 'lv', 'lt',
    'el', 'mt', 'ga', 'tr', 'us'
];

const DEFAULT_LOCALE = 'fr';

let currentLocale = DEFAULT_LOCALE;
let translations = {};

// ─── Initialisation ───

/**
 * Initialise le système i18n :
 * 1. Détecte la langue de l'OS via navigator.language
 * 2. Charge le fichier lang/{code}.json
 * 3. Traduit tous les éléments HTML statiques (data-i18n)
 *
 * Doit être appelé AVANT initLogin(), initEvents(), etc.
 */
async function initI18n() {
    currentLocale = detectLocale();

    // Charger le fichier de traduction
    try {
        const response = await fetch(`lang/${currentLocale}.json`);
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        translations = await response.json();
    } catch (e) {
        console.warn(`i18n: impossible de charger ${currentLocale}.json, fallback sur fr`);
        if (currentLocale !== DEFAULT_LOCALE) {
            currentLocale = DEFAULT_LOCALE;
            try {
                const fallback = await fetch(`lang/${DEFAULT_LOCALE}.json`);
                if (fallback.ok) {
                    translations = await fallback.json();
                }
            } catch (_) {
                console.error('i18n: impossible de charger le fallback fr.json');
                translations = {};
            }
        }
    }

    // Mettre à jour <html lang="xx">
    document.documentElement.lang = getWebLocale();

    // Traduire le DOM statique
    translateDOM();
}

// ─── Détection de la langue OS ───

/**
 * Détecte la langue de l'OS du photographe.
 *
 * navigator.language retourne par ex. "fr-FR", "en-US", "de-DE".
 * On garde la langue, avant le premier tiret, et on cherche dans
 * SUPPORTED_LOCALES.
 *
 * Cas spéciaux (0.3.7 pour les deux derniers) :
 *   - "en-US" → "us" (Anglais US distinct de "en" British) ; tout autre "en-*"
 *     (en-GB, en-AU, en-CA) → "en" ;
 *   - "pt-BR" → "pt_BR" (Brésil, distinct du Portugal) ; tout autre "pt-*" →
 *     "pt" ;
 *   - "nb" et "nn" (Windows en norvégien) → "no".
 */
function detectLocale() {
    return localeDepuisNavigateur(navigator.language || navigator.userLanguage || 'fr');
}

function localeDepuisNavigateur(langue) {
    const parties = String(langue).toLowerCase().replace(/_/g, '-').split('-');
    const code = parties[0];

    if (code === 'en' && parties.includes('us')) {
        return 'us';
    }

    if (code === 'pt' && parties.includes('br')) {
        return 'pt_BR';
    }

    if (code === 'nb' || code === 'nn') {
        return 'no';
    }

    return SUPPORTED_LOCALES.includes(code) ? code : DEFAULT_LOCALE;
}

// ─── Fonction de traduction principale ───

/**
 * Traduit une clé avec interpolation optionnelle.
 *
 * Exemples :
 *   t('login.submit')                          → "Se connecter"
 *   t('upload.success', {duration: '2.3', speed: '450'})
 *                                               → "OK en 2.3s (450 Ko/s)"
 *   t('events.photos_other', {count: 42})       → "42 photos en ligne"
 *
 * Si la clé n'existe pas, retourne la clé elle-même (aide au debug).
 *
 * @param {string} key    Chemin à points dans le JSON (ex: "login.submit")
 * @param {Object} params Valeurs d'interpolation (ex: {count: 5})
 * @returns {string}
 */
function t(key, params = {}) {
    const keys = key.split('.');
    let value = translations;

    for (const k of keys) {
        if (value && typeof value === 'object' && k in value) {
            value = value[k];
        } else {
            // Clé non trouvée
            console.warn(`i18n: clé manquante "${key}" [${currentLocale}]`);
            return key;
        }
    }

    if (typeof value !== 'string') {
        return key;
    }

    // Interpolation : remplacer {name} par params.name
    return value.replace(/\{(\w+)\}/g, (match, name) => {
        return params[name] !== undefined ? params[name] : match;
    });
}

/**
 * Traduction avec pluriel simple (singulier / pluriel).
 *
 * Utilise la convention :
 *   keyBase + "_one"   → forme singulière (count === 1)
 *   keyBase + "_other" → forme plurielle  (count !== 1)
 *
 * Exemple :
 *   tPlural('events.photos', 42) → t('events.photos_other', {count: 42})
 *   tPlural('events.photos', 1)  → t('events.photos_one', {count: 1})
 *
 * @param {string} keyBase Clé de base sans suffixe
 * @param {number} count   Nombre pour le pluriel
 * @param {Object} params  Paramètres additionnels
 * @returns {string}
 */
function tPlural(keyBase, count, params = {}) {
    const suffix = count === 1 ? '_one' : '_other';
    return t(keyBase + suffix, { count, ...params });
}

// ─── Traduction du DOM statique ───

/**
 * Traduit tous les éléments HTML ayant des attributs data-i18n.
 *
 * Attributs reconnus :
 *   data-i18n="key"             → remplace textContent
 *   data-i18n-placeholder="key" → remplace placeholder
 *   data-i18n-title="key"       → remplace title (tooltip)
 */
function translateDOM() {
    // Textes (textContent)
    document.querySelectorAll('[data-i18n]').forEach(el => {
        const key = el.getAttribute('data-i18n');
        const translated = t(key);
        if (translated !== key) {
            el.textContent = translated;
        }
    });

    // Placeholders
    document.querySelectorAll('[data-i18n-placeholder]').forEach(el => {
        const key = el.getAttribute('data-i18n-placeholder');
        const translated = t(key);
        if (translated !== key) {
            el.placeholder = translated;
        }
    });

    // Titres / tooltips
    document.querySelectorAll('[data-i18n-title]').forEach(el => {
        const key = el.getAttribute('data-i18n-title');
        const translated = t(key);
        if (translated !== key) {
            el.title = translated;
        }
    });
}

/**
 * Retourne la locale active (ex: "fr", "en", "de")
 */
function getCurrentLocale() {
    return currentLocale;
}

/**
 * La locale active sous sa forme web (0.3.7), pour <html lang>, Intl et
 * toLocale… : "pt_BR" fait planter Intl (RangeError), "us" n'est pas une langue.
 */
function getWebLocale() {
    return localeWeb(currentLocale);
}

function localeWeb(locale) {
    if (locale === 'us') {
        return 'en-US';
    }

    return String(locale).replace(/_/g, '-');
}
