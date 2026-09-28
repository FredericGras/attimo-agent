# Agent Terrain 0.3.3 — retours de Fabien sur la 0.3.2

**Date :** 28/09/2026 · **Base :** commit `75e36bd` (0.3.2) · **Serveur :** lu, pas modifié.

**Tests :** `test.bat` passe en entier. Il lance **132 tests Rust** (dont le contrôle des traductions) et, désormais, **9 tests des libellés de l'interface** (Node). Aucun avertissement du compilateur. Les tests passent aussi en édition DEV. Aucun installeur n'a été compilé et rien n'a été déposé.

---

## Ce qui a changé

### 1. Numérotation des clips : le serveur compte aussi

- Au démarrage de la première captation d'une épreuve, l'agent demande au serveur son plus grand numéro de clip. Il le fait une fois par épreuve et par lancement, avec un délai de 5 s au plus.
- Le premier clip prend le numéro suivant le plus grand des trois : celui de l'interface, celui de la file locale et celui du serveur.
- Si le serveur ne répond pas, rien ne change : l'agent se comporte comme en 0.3.2 et écrit une ligne dans le journal technique.

⚠️ **Le serveur ne sait pas encore répondre.** `GET /api/sport/events/{id}/clips/status` exige un `session_id` et ne renvoie que les clips de **cette** session (`SportClipUploadController::status`, validation `session_id: required`). Le « recoupement » de la 0.3.2 marche session par session. Il ne peut donc rien dire d'un autre poste ou d'une installation neuve, qui ne connaissent pas les sessions déjà envoyées.

L'agent interroge `clips/status` **sans** `session_id`. Aujourd'hui, le serveur refuse (422) et l'agent garde son comportement. Voir « Ce qui attend le serveur ».

### 2. Photo supprimée de la galerie : le serveur fait foi (prêt, en attente du serveur)

**Au scan**, par groupes de 64 photos, l'agent calcule les empreintes MD5 et interroge `POST /api/sport/events/{eventId}/hash-check`. Il envoie `{"hashes": [...]}` (2 000 empreintes au plus par requête) et lit `{"success":true,"existing":[...]}`.

| Le serveur dit… | La mémoire locale… | Résultat |
|---|---|---|
| il l'a | la connaît ou non | Pas d'envoi. La mémoire est complétée si besoin. |
| il ne l'a pas | la connaît (photo supprimée de la galerie) | **Elle repart.** La mémoire est corrigée. Le journal affiche « Absente de la galerie — renvoyée ». |
| il ne l'a pas | ne la connaît pas | Envoi normal. |
| ne répond pas | — | Comportement 0.3.2 : la mémoire locale seule décide. |

- **Photos détectées en direct :** l'agent n'interroge le serveur que si la mémoire locale croit la photo déjà en ligne. Il le fait pour elle seule.
- **Route absente** (404 ou 405, le cas aujourd'hui) : l'agent ne la redemande plus de la session et ne calcule plus d'empreintes au scan. Rien à l'écran, une seule ligne au journal technique.
- **Erreur réseau :** même repli, pour le groupe concerné.
- **Bonus :** au scan, une photo déjà en ligne n'est même plus relue par l'envoi, puisque son empreinte arrive avec elle.

### 3. Mesure « réseau + serveur »

- La décision est prise par l'agent (`format_mesure`) et testée.
- Le format « réseau Y s + serveur Z s » n'apparaît que si le temps réseau mesuré représente **au moins 30 % du total** et que les deux parts tiennent dans le total.
- Sinon, l'agent affiche « OK en X s (serveur Z s) ». Le cas relevé par Fabien (« réseau 0,0 s + serveur 2,1 s » pour 2,6 Mo) donne désormais « OK en 2,6 s (serveur 2,1 s) ».

### 4. Libellés et affichage

**Bloc HD après une purge**
- Il affiche « 24 envoyés — 5 introuvables sur le disque (restent en version légère) » et « 24 / 29 clips HD envoyés », au lieu de « 24 / 24 — Tous les clips HD sont envoyés ».
- Le message de purge apparaît aussi en tête du bloc vidéo, pas seulement dans le journal.

**Envoi HD**
- La ligne d'avancement affiche le débit mesuré et le temps restant, par exemple « 3 / 7 clips HD envoyés · 28,4 Mb/s · reste ≈ 4 min ».
- Le débit est mesuré sur la dernière minute, d'après les octets HD réellement envoyés, morceau par morceau.

**HD automatique**
- Quand la pleine qualité est activée pendant une captation, le bloc HD l'indique : « HD envoyée automatiquement : la pleine qualité part pendant la course (attention au forfait 4G). »

**Écran « Choisissez votre événement »**
- Il affiche « N vidéos en ligne » et « Identification : mixte dossard + visage » (ou dossards (OCR), visages, liste des inscrits, véhicules, aucune), **si** `GET /api/sport/events` renvoie ces champs.
- Aujourd'hui, le serveur ne les renvoie pas : rien ne s'affiche, et il n'y a aucune erreur.

**Un dossier `video_…` par captation**
- Au démarrage de chaque captation (première, reprise après pause, relance), le journal donne son dossier : « Dossier de cette captation : D:\Videos\video_1727… ».
- Je n'ai pas regroupé les dossiers sous un dossier de session. La file HD mémorise des chemins complets, donc déplacer des dossiers existants n'est pas sans risque. Changer l'arborescence pour les nouvelles captations seulement n'apporterait qu'une différence de rangement.

**Manifeste d'une galerie « Aucune »**
- `"analyse": {"actif": false, "motif": "galerie sans identification", …}`.
- Il est écrit dès la création si l'épreuve est déjà connue sans identification, sinon dès que la sonde ou le premier refus du serveur le révèle.

### 5. Journal technique

- **Dernier morceau trop court à la pause ou à l'arrêt** (moins de 2 s) : l'agent n'en extrait plus d'images. Il écrit une ligne INFO (« Dernier morceau 12 : 0,6 s, trop court pour des images d'analyse ») au lieu de l'erreur FFmpeg -22 et de son rapport complet. Pour un morceau plus long qui ne donne tout de même aucune image, il écrit une ligne INFO sans le rapport FFmpeg.
- **Arrêt d'une session pendant un scan :** « Scan interrompu, session close : IMG_…JPG n'est pas confiée à l'envoi » s'écrit désormais en INFO, au lieu de « Erreur envoi vers uploader: channel closed ».

### Règles du lot

- **Version :** 0.3.3 dans `tauri.conf.json`, `Cargo.toml`, `Cargo.lock`, `package.json` et `package-lock.json`.
- **Traductions :** 18 nouveaux textes dans les 27 langues. Le contrôle `traductions.rs` est vert et couvre aussi `affichage.js`.
- **Édition DEV :** rien n'a changé dans l'installation côte à côte (nom, identifiant, dossier de données).
- **Nouveaux fichiers :**
  - `src-tauri/src/verification.rs` : le hash-check et un faux serveur HTTP local pour les tests ;
  - `src/affichage.js` : les libellés, sans accès à l'écran ;
  - `tests/affichage.test.cjs` : les tests Node, lancés par `test.bat` si Node est installé.
- **Tests ajoutés :**
  - numéro de clip du serveur (réponse avec `clips`, avec `max_clip_index`, refus 422 d'aujourd'hui) ;
  - hash-check présent, absent (404 et 405), serveur injoignable, réponse illisible ;
  - règles « le serveur fait foi » et « sans serveur, comportement 0.3.2 » ;
  - règle des 30 % ;
  - libellés du bloc HD (purge, débit, temps restant) et de l'écran des épreuves (champs présents ou absents) ;
  - manifeste « Aucune » et dernier morceau trop court.

---

## Ce qui attend le serveur

1. **Hash-check pour l'agent** (point 2) : `POST /api/sport/events/{eventId}/hash-check`.
   - Groupe `app-password:tauri_agent` de `routes/api.php`, avec le scope tenant.
   - Même logique que `SportUploadChunkController::hashCheck`, via `SportChunkService::hashExistsForEvent`.
   - Entrée : JSON `{"hashes": ["<md5>", ...]}`, 2 000 au plus. Sortie : `{"success": true, "existing": ["<md5>", ...]}`.
   - L'agent l'utilisera dès sa mise en ligne, sans nouvelle version.
2. **Plus grand numéro de clip de l'épreuve** (point 1). `GET /api/sport/events/{id}/clips/status` doit accepter l'absence de `session_id`. Deux réponses conviennent, au choix :
   - `{"success": true, "clips": [{"index": …}, …]}` pour toute la galerie de l'épreuve ;
   - ou simplement `{"success": true, "max_clip_index": 41}`.

   L'agent lit les deux formes.
3. **Champs de `GET /api/sport/events`** (point 4), dans `SportAgentController::events` :
   - `video_count` : le nombre de vidéos en ligne. L'agent accepte aussi le nom `videos_count`.
   - `recognition_mode` : `bib`, `face`, `mixed`, `roster`, `vehicle` ou `none`.

   Ils sont facultatifs : sans eux, l'écran reste comme aujourd'hui.

---

## Protocole de test pour Fabien (agent DEV 0.3.3)

1. **Installation :** le badge affiche « v0.3.3 DEV ». La connexion est reprise sans rien ressaisir.
2. **Mesure (point 3) :** en 4G, envoyer une vingtaine de photos. Aucune ligne ne doit afficher « réseau 0,0 s ». Les lignes sont soit « OK en X s (serveur Y s) », soit « réseau Y s + serveur Z s » avec Y d'au moins un tiers de X environ.
3. **Renvoi du dossier (point 2, serveur actuel) :** renvoyer un dossier déjà en ligne. Comme en 0.3.2, le journal affiche « Déjà en ligne — non renvoyée ». Aucun message d'erreur à l'écran. Le journal exporté contient **une seule** ligne « Vérification des empreintes par le serveur indisponible… ».
4. **Bloc HD (point 4) :**
   1. Filmer 3 min sans pleine qualité, puis terminer la session.
   2. Supprimer le dossier `clips` d'une captation.
   3. Rouvrir l'épreuve et cliquer « Envoyer les HD maintenant ». Le bloc vidéo affiche en tête « … retiré(s) de la file … ». Pendant l'envoi, la ligne d'avancement montre un débit et « reste ≈ … ». À la fin, le bloc affiche « N envoyés — M introuvables sur le disque (restent en version légère) ».
5. **HD automatique :** cocher « Envoyer la pleine qualité pendant la course » et filmer 1 min. Le bloc HD affiche « HD envoyée automatiquement… ».
6. **Dossiers et journal (points 4 et 5) :**
   1. Filmer, faire une pause, reprendre, arrêter, puis relancer. Chaque démarrage écrit « Dossier de cette captation : … », avec un dossier différent à chaque fois.
   2. Exporter le journal. Il ne contient plus « Aucune image extraite du morceau » avec le rapport FFmpeg, mais au plus une ligne INFO « trop court pour des images d'analyse ».
7. **Galerie « Aucune » :** filmer 1 min, puis ouvrir `manifest.json` dans le dossier `video_…`. Il contient `"actif": false` et `"motif": "galerie sans identification"`.
8. **Écran des épreuves :** il est inchangé et sans erreur, tant que le serveur n'envoie pas les nouveaux champs.
9. **Numérotation (point 1) :** sur le même poste, aucun changement visible : les numéros continuent comme en 0.3.2. Le test « autre PC » ne sera possible qu'après la modification serveur 2.
