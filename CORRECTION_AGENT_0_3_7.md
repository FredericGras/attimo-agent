# Agent Terrain 0.3.7 — portugais du Brésil et messages d'erreur traduits

**Date :** 05/11/2026 · **Base :** commit `a586928` (0.3.6) · **Serveur :** aucun changement demandé.

**Source :** demande de Frédéric : ajouter `pt_BR` à l'agent, comme à Attimo Gallery, et ne plus laisser de texte en français dans les autres langues.

**Tests :** 161 tests Rust (152 avant, 9 nouveaux) et 23 tests d'interface (Node, 18 avant). Tous passent, sans avertissement du compilateur. Le contrôle des traductions est vert (28 langues). Les nouvelles gardes ont été vues rouges sur des traductions cassées exprès, puis les fichiers remis à l'identique.

**Installateur :** DEV seulement (`build-preprod.bat`). La production ne bouge pas : Fabien garde sa version tant que personne n'a lancé `build-prod.bat`.

**État de départ :** aucun fichier suivi n'était modifié. Les deux fichiers non suivis `DIAGNOSTIC_DEBIT_AGENT.md` et `attimo-agent-debit-envoi-brief.md` sont toujours là. Je n'y ai pas touché et ils ne sont pas dans le commit.

---

## Ce qui n'allait pas

1. **Le Brésil n'existait pas.** L'agent avait 27 langues. Un Windows en `pt-BR` recevait le portugais du Portugal (`ficheiro`, `palavra-passe`…).
2. **La détection ne pouvait pas le voir.** Elle coupait la langue à 2 lettres : `pt-BR` devenait `pt`, et `nb-NO` (Windows en norvégien) devenait `nb`, inconnu, donc le français. `fil-PH` devenait `fi`, le finnois.
3. **Les erreurs restaient en français.** Elles étaient écrites en français dans le Rust et affichées telles quelles. Un photographe allemand lisait « Erreur : Espace disque insuffisant… ». Certaines portaient même le rapport technique de FFmpeg, ou une erreur réseau en anglais.
4. **« Surveillance démarrée pour… » et « LIVE »** étaient écrits en dur.

## Corrections

**Langue (`i18n.js`)**
- `pt-BR` → `pt_BR` ; toute autre forme de portugais → `pt`.
- `nb` et `nn` → `no`.
- La langue se lit avant le premier tiret, et non sur 2 lettres.
- `<html lang>` et l'heure du journal reçoivent la forme web (`pt-BR`, `en-US`). `pt_BR` brut fait planter `toLocaleTimeString` : sans ce point, **chaque ligne du journal aurait planté** sur un poste brésilien.

**`src/lang/pt_BR.json`** : traduit depuis le français, en entier. « você », tela, arquivo, usuário, senha, equipe… Un test refuse les mots du Portugal.

**Erreurs : un code, pas une phrase**
- Le Rust renvoie un code : `err.network`, ou `err.folder_missing|{"folder":"D:\\Photos"}` avec ses variables (`src-tauri/src/erreurs.rs`).
- L'écran le traduit avant de l'afficher (`Affichage.messageErreur`, `texteErreur` dans `main.js`).
- Le détail technique (FFmpeg, erreur système) part au journal de l'agent, plus à l'écran.
- Un texte inconnu ne s'affiche jamais tel quel : message générique traduit à l'écran, texte d'origine au journal sur le disque.
- Le message d'un refus du serveur reste affiché, encadré d'un texte traduit : « Réponse du serveur : Event not found. (404) ». Il vient du serveur, en anglais aujourd'hui ; le traduire est un autre chantier, côté serveur.

**Ce qui n'a pas changé, vérifié dans le code**
- Les codes qui pilotent l'agent : `SESSION_EXPIRED`, `ARRET_EN_COURS`, `EPREUVE_BROUILLON`, `ANALYSE_DESACTIVEE`, `SATURE:` et `DEFINITIF:` gardent leur valeur et leurs comparaisons.
- Une seule comparaison portait sur une phrase : `includes('Aucune captation')` dans `main.js`. Elle compare maintenant `err.no_recording`.
- Envois, reprise, file vidéo, nombre d'essais : aucune logique touchée.

**Laissé en français, volontairement**
- Le journal technique : `info!`, `warn!`, `error!`, l'en-tête de l'export du journal.
- Les erreurs qui ne vont qu'au journal : `secret.rs`, `auth.rs` (`save_auth`), `uploader.rs` (`create_http_client`), `verification.rs`, l'état interne de `journal.rs`.
- Les valeurs écrites en base locale et jamais relues par l'écran (`last_error` : « Fichier introuvable », « Analyse refusée », « Arrêt pendant le retry »…).
- Le motif écrit au manifeste (`galerie sans identification`) : une donnée, pas un affichage.
- Les noms propres : ATTIMO, FFmpeg, Attimo Agent Terrain.

**Choix fait seul, à relire si besoin**
- « LIVE » devient « En direct » en français (et « Ao vivo », « Live », « En directo »… ailleurs). Une tuile française change donc de libellé.
- Les erreurs voisines ont été regroupées sous un même code (par exemple, tous les dossiers impossibles à créer sous `err.create_folder`) : 41 messages au lieu d'une centaine. Le journal garde la précision.

## Fichiers

| Fichier | Changement |
|---|---|
| `erreurs.rs` | Nouveau : codes, variables, détail au journal. |
| `auth`, `commands`, `uploader`, `video_uploader`, `video_queue`, `watcher`, `recorder`, `capture`, `frames`, `assembler`, `disk`, `manifest` | Les erreurs qui atteignent l'écran passent par un code. |
| `traductions.rs` | 28 langues ; 5 gardes nouvelles (voir plus bas). |
| `i18n.js` | 28 langues, détection, forme web de la langue. |
| `affichage.js`, `main.js` | Traduction des erreurs, « En direct », « Surveillance démarrée ». |
| `src/lang/*.json` | `pt_BR.json` nouveau ; 43 clés ajoutées dans les 27 autres (`events.live`, `dashboard.msg_started`, section `err`). |
| `tests/affichage.test.cjs` | 5 tests nouveaux : erreurs et détection de la langue. |
| Version | 0.3.7 dans `tauri.conf.json`, `Cargo.toml`, `Cargo.lock`, `package.json` et `package-lock.json`. |

## Vérifications

Gardes ajoutées à `traductions.rs` :
- chaque code `err.…` écrit dans le Rust existe dans `fr.json` ;
- chaque code reçoit exactement les variables de son texte ;
- aucun message d'erreur n'est resté en français dans une autre langue ;
- chaque langue de `src/lang` est proposée par `i18n.js` ;
- le portugais du Brésil ne contient aucun mot du Portugal, et n'est pas une copie de `pt`.

**Pas testé :** l'agent lancé pour de vrai à l'écran, sur un Windows en `pt-BR` ou `nb-NO`. La détection et la forme web sont testées hors de l'agent (Node). Les traductions n'ont été relues par aucun locuteur natif.

---

## Recette pour Fabien — agent DEV 0.3.7, sur dev

1. **Installation.** Installer le `.msi` DEV : le badge affiche « v0.3.7 DEV » et la connexion est conservée.
2. **Français, non-régression.**
   - Photos et vidéo sur une épreuve : les envois partent comme en 0.3.6.
   - Le journal écrit « Surveillance démarrée pour « … » ».
   - Une épreuve en direct porte « En direct » au lieu de « LIVE ».
3. **Messages d'erreur.**
   - Coller un mot de passe d'application faux : « Format de mot de passe invalide… ».
   - Couper le réseau, puis ouvrir la liste des épreuves : « Impossible de contacter le serveur Attimo… ».
   - Exporter le journal : les détails techniques y figurent.
4. **Autre langue.** Passer Windows en portugais (Brésil), relancer l'agent : tout l'écran est en brésilien, « tela », « arquivo », et l'heure du journal s'affiche.
