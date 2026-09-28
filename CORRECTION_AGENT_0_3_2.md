# Agent Terrain 0.3.2 — corrections après les tests du 28/09

**Date :** 28/09/2026 · **Base :** commit `2c8862a` (0.3.1) · **Périmètre :** agent uniquement. Le dossier `C:\laragon\www\attimo` a seulement été lu.

**Sources :** `attimo-agent-v031-compte-rendu-2026-09-28_1.md` (tests de Fabien, dans `C:\laragon\www\attimo`) et `CORRECTION_AGENT_0_3_1.md`. Hors lot, comme demandé : OCR des dossards à 4 chiffres et « IA » sur les galeries « Aucune » (B1).

**Rien n'a été compilé pour livraison ni déposé.** Seuls `cargo test` et `cargo check` ont tourné, en mode debug, sans installeur. Résultat : **118 tests, tous verts, aucun avertissement du compilateur**, aussi avec `ATTIMO_EDITION=dev`.

> ⚠️ **Disque C: plein.** La compilation des tests a échoué une première fois faute de place (901 Mo libres). J'ai supprimé `src-tauri\target\debug\incremental` (4,3 Go). C'est un cache de compilation, recréé automatiquement. Il reste **4 Go libres** : c'est juste pour `build-preprod.bat`. Libère de la place avant de compiler.

---

## Vidéo

### V1. Aucun clip n'est envoyé pendant la captation

**Cause.** C'était la file d'envoi, pas la priorité aux photos.
- `video_queue::prochains` trie par nature : les images d'analyse passent d'abord, puis les clips légers, puis la HD.
- `reserver_prochain_envoi` ne choisissait **aucun clip tant qu'il restait une image en file**.
- Pendant la captation, une image arrive toutes les 2 s. Le serveur analyse chaque image d'un lot avant de répondre (Rekognition : dossards et visages). La file d'images ne se vidait donc jamais tant que la caméra tournait.
- Sur une galerie « Aucune », c'était pire : chaque lot était refusé trois fois (voir V4).
- Les clips ne partaient qu'une fois la caméra arrêtée et le retard d'images résorbé : 3 à 4 minutes plus tard, comme l'a mesuré Fabien.

Les deux autres pistes sont écartées :
- **La priorité aux photos rend bien la main.** Le compteur est équilibré, et l'attente est plafonnée à 20 s par requête.
- **Les clips ne sont pas retenus jusqu'à l'arrêt.** Ils entrent en file dès que leur version légère est prête.

**Correction.**
- **Deux voies d'envoi.** L'un des deux envois vidéo sert d'abord les clips (légers, puis HD si elle est autorisée), l'autre sert d'abord les images. Une voie sans travail prend celui de l'autre. Les clips légers partent donc au fil de la captation, quel que soit le retard des images.
- **Priorité aux photos allégée pour les clips légers.** La HD continue de céder la place aux photos à chaque morceau de 5 Mo. Un clip léger (quelques Mo) ne cède qu'une fois, avant son premier morceau.
- Fichiers : `video_queue.rs` (`Voie`, `reserver_prochain_envoi`), `commands.rs` (`process_video_queue`, paramètre `voie`), `video_uploader.rs`, et `main.js` (un ouvrier par voie).

### V2. Compteurs faux

**Cause.** Les compteurs lisaient bien la file, mais trois choses les faisaient paraître faux :
1. **L'effet de V1.** Pendant la captation, « 0 envoyé / 23 en attente » décrivait fidèlement une file bloquée.
2. **« Envoyés » ne comptait que les versions légères de la session en cours.** La galerie, elle, montre tous les clips de l'épreuve. Une session précédente, ou une reprise après avoir terminé la session (fréquent tant que la relance était impossible), donnait « 7 envoyés » face à 19 en ligne.
3. **Les abandons restaient mêlés à l'attente.**

**Correction.**
- « Clips envoyés » compte les clips **confirmés par le serveur** : sa réponse à la finalisation, ou le recoupement `clips/status` à la reprise. Un clip n'est compté qu'une fois, qu'il ait sa version légère, sa HD, ou les deux (`clips_online`).
- Si l'épreuve compte plus de clips en ligne que la session, le libellé l'affiche : « clips envoyés — 19 en ligne sur l'épreuve ».
- « En attente » ne compte que les versions légères en file ou en cours d'envoi. Les échecs sont affichés à part : « clips en attente d'envoi — 2 en échec ».

### V3. Numérotation remise à clip_0001 après une pause

**Cause.** Chaque reprise ouvre une nouvelle captation. Son suivi de clips (`ClipTracker`) repartait de 1, et ce numéro sert à la fois au nom de fichier et au `clip_index` envoyé au serveur.

**Correction.**
- **La numérotation continue sur toute l'épreuve** : pauses, reprises, relance, et même une nouvelle session sur la même épreuve. Après clip_0007 vient clip_0008.
- Le numéro de départ est le plus grand de deux valeurs : celle que tient l'interface (renvoyée par l'arrêt, `prochain_numero`), et le premier numéro libre de l'épreuve dans la file locale.
- L'interface retient ce numéro parce que le dernier clip n'entre en file qu'après sa version légère, plusieurs secondes après l'arrêt. Une reprise immédiate ne reprend donc jamais son numéro.
- Les horodatages restent calculés depuis le début de chaque captation. Seul le numéro change.

**Appariement HD ↔ aperçu, vérifié dans le code serveur.**
- La clé d'appariement est tenant + galerie + `sport_clip_session_id` + `sport_clip_index` (`SportClipUploadService::findOrCreateClip`, l. 125-157). Les deux variantes d'un clip portent toujours la même session et le même numéro.
- Des numéros qui continuent d'une captation à l'autre ne posent aucun problème : le serveur ne valide ni la continuité ni le départ à 1. Mieux, `sort_order` = numéro, donc les clips s'affichent dans l'ordre global.
- **Deux durcissements côté agent :**
  - **Les deux variantes d'un même clip ne partent jamais en même temps.** L'index serveur `idx_videos_sport_clip` n'est pas unique : deux finalisations simultanées pouvaient créer deux fiches.
  - **L'identifiant d'envoi des morceaux porte l'épreuve** (`e<épreuve>_<session>_<variante>_<n°>`). Le serveur range les morceaux par cet identifiant seul, tous comptes confondus.

### V4. « ANALYSE_DESACTIVEE » en boucle sur une vidéo sans IA

**Cause.**
- Tout refus 422 du serveur valait « analyse désactivée », mais était traité comme une erreur ordinaire : trois essais par lot, puis abandon.
- Les images continuaient d'être extraites et mises en file.
- La ligne revenait donc à chaque lot.

**Correction.**
- **Une erreur définitive n'est jamais relancée.** Tout refus 4xx du serveur (hors 401, 408, 429) sort l'élément de la file dès le premier refus, avec une seule ligne claire : « Refusé par le serveur, non relancé : Clip 12 — … ».
- Exception : un 422 à la finalisation d'un clip (« envoi incomplet ») garde ses essais, car un nouvel essai renvoie tous les morceaux.
- **Au démarrage de chaque captation, l'agent interroge le serveur** avec une requête d'images vide.
  - Le serveur contrôle l'épreuve **avant** le contenu (`SportClipFrameController::store` appelle `resolveEvent` avant `validate`). Il répond donc « Recognition is disabled for this event. » si la galerie est « Aucune », et sinon refuse la requête faute d'image.
  - Rien n'est analysé ni facturé.
- **Si l'analyse est désactivée :**
  - Le journal le dit une fois : « Galerie sans identification : les images d'analyse ne sont pas envoyées. Les clips partent normalement. »
  - Les images ne sont plus extraites, ce qui épargne le processeur et le disque, et celles déjà en file en sortent.
  - Le compteur affiche « images d'analyse — non envoyées (galerie sans identification) ».
- Si la sonde échoue (réseau), le premier refus du serveur produit exactement le même effet.

### V5. Impossible de relancer la captation après « Arrêter la vidéo »

**Correction.**
- **Confirmation avant l'arrêt** : « Arrêter la captation vidéo ? Les clips déjà filmés continuent de partir, et la captation pourra être relancée. »
- **Bouton « Relancer la captation »** après un arrêt. Il reste dans la même session, avec les réglages du premier démarrage. La numérotation continue et le chrono reprend son cumul.
- Voir aussi P2 : aucune confirmation de l'agent ne fonctionnait jusqu'ici. Celle de l'arrêt aurait été ignorée comme les autres.

### V6. Chrono et « Go écrits » hérités ; compteur disque remis à zéro à chaque reprise

**Cause.**
- Le chrono affiché et le bloc disque n'étaient remis à zéro nulle part au démarrage d'une nouvelle session.
- À l'inverse, chaque reprise écrit dans son propre dossier, et la surveillance disque, relancée avec elle, repartait de zéro.
- Enfin, revalider les réglages après un arrêt vidéo remettait le chrono à zéro.

**Correction.**
- **Nouvelle session :** chrono à 00:00:00, bloc disque vidé.
- **Reprises et relances :** elles cumulent. La surveillance disque indique désormais sa captation (`session_id` dans `disk-status`), et l'écran additionne les octets écrits par toutes les captations de la session.

### V7. File HD non purgée

**Correction.**
- Un élément de la file dont le fichier n'existe plus en sort (statut `missing`), avec une ligne au journal : « 23 fichier(s) vidéo retiré(s) de la file : introuvable(s) sur le disque (clips 1, 2, … 23) ».
- **Moments de contrôle :**
  - à l'ouverture de l'épreuve, avant d'afficher la carte « Clips HD en attente » ;
  - au démarrage de la session ;
  - avant « Envoyer les HD maintenant » ;
  - toutes les 30 s environ ;
  - au moment de réserver chaque envoi.
- Le clic ne produit plus jamais d'erreur sur un fichier absent.

### V8. Images d'analyse en 1920 px : **non appliqué, à trancher**

La vérification demandée dans le code serveur conclut que **1280 px ne suffit pas** :
- **Aucune mise à l'échelle côté serveur.** `SportClipFrameService::processFrame` transmet les octets bruts à Rekognition (`detectText`, confiance minimale 70).
- **Le serveur documente 1920 px comme « non négociable (invariant n°1) »** (`SportClipFrameService.php` l. 26-42), avec la même mesure que l'agent (`frames.rs`) :
  - 720p, soit **1280 px** de large : 3 lectures correctes, **1 erronée** ;
  - 1920 px : 6 lectures correctes, 0 erronée.
- Une lecture erronée envoie les photos au mauvais coureur, ce qui est pire qu'une absence de lecture. C'est d'autant plus sensible avec le problème des dossards à 4 chiffres.

La largeur reste donc à 1920 px. Le raisonnement est noté dans `frames.rs`.

Le gaspillage relevé par Fabien venait surtout des galeries « Aucune » (V5 et V6 du test). Là, **plus aucune image n'est extraite ni envoyée** (voir V4).

Pour passer quand même à 1280 px, il faudrait refaire la mesure côté serveur sur de vrais dossards : 1280 contre 1920, à 5 et à 40 m.

---

## Photos

### P1. Anti-doublon AVANT l'envoi

**Constat serveur.**
- `/api/sport/upload` dédoublonne par **MD5 du fichier entier**, par épreuve (`SportUploadController.php` l. 88-98). Le doublon est écarté après réception, avec `duplicate: true` dans la réponse.
- Il existe une vérification d'empreintes (`POST /dashboard/sport/events/{id}/upload/hash-check`), mais **seulement en route web** : session navigateur et CSRF. L'agent ne peut pas l'utiliser.

**Correction (agent).**
- **Mémoire locale par épreuve** : nouvelle table `photos_envoyees` (épreuve, empreinte MD5, taille, nom, identifiant serveur).
- **Fichier lu une seule fois.** Chaque photo est lue une fois : son MD5 (le même calcul que le serveur) est comparé à la mémoire, puis le même contenu part tel quel. Aucune lecture en plus.
- **Photo déjà connue :** elle ne part pas. Le journal indique « Déjà en ligne — non renvoyée », elle compte dans « Envoyées », et le bilan précise « (dont 215 déjà en ligne, non renvoyées) ».
- La mémoire est alimentée à chaque envoi réussi, y compris quand le serveur répond `duplicate: true`. Le journal le signale alors : « le serveur l'avait déjà ».
- **Portée :** la mémoire vaut quel que soit le dossier ou la session. Elle est propre à l'édition (la DEV a la sienne) et au poste.
- MD5 est implémenté localement (RFC 1321, testé sur le jeu d'essai officiel), sans dépendance à télécharger.
- **Limite :** une photo supprimée de la galerie à la main ne repartira pas depuis ce poste. Le point d'entrée serveur ci-dessous lèverait cette limite.

### P2. « Surveillance démarrée » avant la confirmation

**Cause (générale, grave).**
- Le module de dialogue de Tauri (`tauri-plugin-dialog`, `init-iife.js`) remplace `window.confirm` par une version **asynchrone**, qui rend une promesse.
- `if (!confirm(...))` ne valait donc jamais rien : une promesse est toujours « vraie ». La suite partait sans attendre, **et sans tenir compte de la réponse**.
- Cela touchait les quatre confirmations de l'agent : autre épreuve, envoi HD seul sur une autre épreuve, « Terminer la session » et déconnexion. Un « Non » n'arrêtait rien.

**Correction.** Une fonction `confirmer()` attend la réponse. Toutes les confirmations passent par elle, y compris la nouvelle confirmation d'arrêt vidéo. Rien ne démarre avant le oui.

### P3. Compteurs à « 0 / 0 » au début ; « Terminé » affiché deux fois

**Causes.**
- **Compteurs à 0 / 0.** Ils n'étaient émis qu'à la fin de chaque envoi. Or le premier envoi prend quelques secondes.
- **« Terminé » en double.** Deux envois qui finissent ensemble voyaient tous deux la file vide, et chacun annonçait « Terminé ».

**Correction.**
- Les compteurs suivent la détection : émis à chaque photo inscrite (au plus toutes les 300 ms) et à la fin de chaque paquet du scan.
- Un bilan « Terminé » n'est annoncé qu'une fois. Un nouveau n'apparaît que si de nouvelles photos ont été traitées depuis.

### P4. « réseau 0,0 s + serveur 2,7 s »

**Cause.**
- L'agent calculait réseau = total − Server-Timing.
- Or le serveur mesure à partir de `REQUEST_TIME_FLOAT` (`SportUploadController.php` l. 52). Sous Apache + PHP-FPM, ce chrono démarre quand PHP reçoit la requête, **avant la fin de la réception du fichier**.
- Le transfert était donc compté dans « serveur ».

**Correction.**
- L'agent mesure lui-même l'envoi : le fichier part en flux, par morceaux de 64 Ko, et l'agent note l'instant où le dernier octet est remis au réseau.
- **Affichage :**
  - si envoi + serveur tiennent dans le total, les deux mesures ne se recouvrent pas : « OK en 3,1 s — réseau 2,8 s + serveur 0,3 s » ;
  - sinon, le temps serveur couvre aussi le transfert : « OK en 3,1 s (serveur 2,7 s) », comme demandé ;
  - le temps d'envoi mesuré est toujours écrit dans le journal sur disque.
- Avec le serveur actuel, attends-toi surtout au second format, en 4G. Il deviendra le premier dès la correction serveur 2 ci-dessous, sans nouvelle version de l'agent.

---

## Divers

### D1. Journal persistant et « Exporter le journal »

- **Fichier sur disque :** `%APPDATA%\com.attimo-gallery.agent[.dev]\journaux\agent.log`. Il contient le journal technique de l'agent **et** chaque ligne affichée à l'écran (envoyées par paquets, une fois par seconde).
- **Rotation** à 5 Mo, cinq fichiers au plus (25 Mo en tout).
- **Bouton « Exporter le journal »** dans l'en-tête du journal de surveillance, et sur l'écran des épreuves pour un export hors session. Il recolle tous les fichiers, du plus ancien au plus récent, dans un fichier choisi par le photographe.
- **À l'écran :** 5 000 lignes au lieu de 200. Une manche de 215 photos en produit environ 700.

### D2. Même dossier pour la vidéo et les photos

L'agent alerte si les deux dossiers sont identiques ou si l'un contient l'autre (sans tenir compte des majuscules ni de / et \) :
- **dans les réglages**, un avertissement sous « Dossier d'enregistrement » ;
- **au démarrage**, une ligne au journal.

Rien n'est bloqué. La surveillance photo n'est pas récursive, les images d'analyse (dans `video_…\_analyse`) ne partent donc pas comme des photos.

### D3. Jeton d'accès chiffré

- **Windows :** DPAPI (`CryptProtectData`), lié à la session Windows de l'utilisateur, avec une entropie propre à l'agent. `auth.json` ne contient plus que le nom, l'adresse et le jeton chiffré (`token_chiffre`). Copié sur un autre poste ou lu par un autre compte, il est inutilisable.
- **macOS :** trousseau (Keychain). ⚠️ Ce code n'a **pas pu être compilé ici** (pas de Mac). Il faut le vérifier au prochain build GitHub macOS.
- **Connexion existante reprise sans redemander le mot de passe.** Un ancien `auth.json` en clair est relu une dernière fois, puis réécrit chiffré au lancement.
- Si le coffre est indisponible, la connexion vaut pour la séance sans être retenue. L'agent n'écrit jamais le jeton en clair à la place.

---

## Pour tout le lot

- **Version 0.3.2** dans `tauri.conf.json`, `Cargo.toml` (et `Cargo.lock`), `package.json`, `package-lock.json`.
- **L'édition DEV reste installable à côté de la production.** Rien n'a changé dans `build-preprod.bat`, `tauri.preprod.conf.json` ni le dossier de données. La nouvelle mémoire des empreintes et le journal vivent dans le dossier propre à chaque édition.
- **Langues :** 21 nouveaux textes dans les 27 langues, avec le même registre que l'existant.
- **Contrôle automatique des traductions** (`traductions.rs`, lancé par `test.bat`). Il vérifie que :
  - chaque langue a toutes les clés du français, et aucune de plus ;
  - les variables `{…}` sont les mêmes ;
  - chaque clé appelée par `main.js` et `index.html` existe.
- **Tests ajoutés (24) :**
  - voies d'envoi, et deux variantes d'un clip jamais envoyées ensemble ;
  - échec définitif, et images d'une galerie « Aucune » écartées ;
  - purge des fichiers absents ;
  - compteur « en ligne » ;
  - numérotation continue (file et suivi de clips) ;
  - lecture de la sonde d'analyse et identifiant d'envoi ;
  - MD5 (RFC 1321) et mémoire des empreintes par épreuve ;
  - bilan « Terminé » unique ;
  - rotation du journal, DPAPI, et traductions.
- **Rien n'a été retiré.** Les anciennes clés de traduction restent. `upload_frames` et `envoyer_images` sont conservés.

---

## Ce qui touche le serveur (rien n'a été modifié)

1. **P1 — Point d'entrée manquant : vérification d'empreintes pour l'agent.**
   - **Route :** `POST /api/sport/events/{eventId}/hash-check`, dans le groupe `app-password:tauri_agent` de `routes/api.php` (à côté des routes `clips`, l. 65-85).
   - **Contenu :** la logique de `SportUploadChunkController::hashCheck` (`SportUploadChunkController.php` l. 49-73), c'est-à-dire `SportChunkService::hashExistsForEvent` (`SportChunkService.php` l. 68-82), avec le scope tenant.
   - **Entrée :** `hashes[]`, des MD5 de 32 caractères, 2 000 au plus. **Sortie :** `{"success":true,"existing":[…]}`.
   - L'agent calcule déjà ces empreintes : il pourrait interroger le serveur par lots au scan, ce qui couvrirait un autre poste ou une réinstallation.
2. **P4 — Server-Timing.**
   - `SportUploadController.php` l. 52 part de `REQUEST_TIME_FLOAT`, qui inclut la réception du fichier sous Apache + FPM.
   - Remplacer par `LARAVEL_START` (défini dans `public/index.php`, après que PHP a lu tout le corps), ou par un `microtime(true)` en entrée de contrôleur.
   - L'agent affichera alors « réseau + serveur » de lui-même.
3. **V3 — Doublon de fiche possible.**
   - L'index `idx_videos_sport_clip` (migration `2026_08_13_100000…`, l. 67-73) n'est pas unique, et `findOrCreateClip` n'a pas de verrou.
   - L'agent n'envoie plus les deux variantes d'un clip en même temps. Un index unique (tenant, galerie, session, numéro), ou un `Cache::lock` dans `findOrCreateClip`, fermerait la porte côté serveur.
4. **Bug relevé (important) : identifications vidéo qui passent d'une épreuve à l'autre.**
   - `SportClipFrameService::attachPendingIdentifications` (l. 564-593) prend `SportVideoIdentification::awaitingClip()->withinWindow($start, $end)` **sans filtre d'épreuve ni de tenant** (scopes `SportVideoIdentification.php` l. 132-148).
   - Un clip peut donc récupérer des dossards lus sur une autre épreuve filmée à la même heure.
5. **V4 — Mode d'identification absent de `GET /api/sport/events`** (`SportAgentController.php` l. 95-109). Ajouter `recognition_mode` éviterait la sonde au démarrage de la captation.
6. **Réponse des morceaux de clip toujours vide.**
   - `SportClipUploadController.php` l. 78-80 lit `received`, `total` et `complete`, alors que `VideoUploadService::storeChunk` (l. 176-182) renvoie `received_count`, `total_chunks` et `is_complete`.
   - C'est sans effet sur l'agent, qui ne lit que `success`.
7. **Rappel 0.3.1 :** Server-Timing est absent de `clips/chunk`, `clips/finalize` et `frames`. Les images sont analysées dans la requête, avant la réponse.

---

## Protocole de test pour Fabien (agent DEV 0.3.2, serveur dev-saas)

**0. Installation**
1. Fred libère de la place sur C:, lance `build-preprod.bat` et installe le `.msi` DEV.
2. Le badge affiche **« v0.3.2 DEV »**.
3. L'agent s'ouvre **sans redemander** le mot de passe d'application (D3).
4. Ouvrir `%APPDATA%\com.attimo-gallery.agent.dev\auth.json` : il ne contient plus `attimo_pat_…` en clair, mais `token_chiffre`.

**1. Confirmations (P2)**
1. Session sur la galerie A, puis ouvrir la galerie B et cliquer « Démarrer ». Répondre **Non** : la session A continue, et rien n'apparaît au journal.
2. Recommencer et répondre **Oui** : A se termine, puis B démarre.

**2. Anti-doublon (P1, P3)**
1. Envoyer les 215 photos sur une galerie.
2. Terminer la session, puis en redémarrer une sur **la même galerie et le même dossier**, avec « Uploader les fichiers existants ».
3. Attendu :
   - les compteurs bougent dès la détection ;
   - le journal affiche « Déjà en ligne — non renvoyée » ×215, en quelques secondes ;
   - « Mb/s envoyés » reste à « — » ;
   - un seul « Terminé : 215 photos envoyées avec succès ! (dont 215 déjà en ligne, non renvoyées) ».
4. Sur fast.com ou sur le routeur, vérifier qu'aucun volume notable n'est parti.

**3. Mesure d'envoi (P4)**

En 4G, relever quelques lignes « OK en X s … ». Deux formats sont possibles :
- « réseau Y s + serveur Z s », si les deux mesures tiennent dans le total ;
- sinon « OK en X s (serveur Z s) ».

Le « réseau 0,0 s » ne doit plus apparaître.

**4. Vidéo pendant la captation (V1, V2)**

Galerie **avec** identification, captation de 10 min, avec des photos au début :
- les lignes « Clip N envoyé » arrivent **pendant** la captation, 1 à 2 min après « Clip N assemblé » au plus ;
- « clips envoyés » monte en même temps que la galerie ;
- « en attente » reste bas.

**5. Galerie sans identification (V4, V8)**

Galerie « Aucune », captation de 3 min :
- **une seule** ligne « Galerie sans identification : les images d'analyse ne sont pas envoyées… », au démarrage ;
- aucune ligne « ANALYSE_DESACTIVEE » ;
- le compteur affiche « images d'analyse — non envoyées » ;
- les clips partent pendant la captation.

**6. Pause, arrêt, relance, numérotation (V3, V5, V6)**
1. Filmer 2 min, **Pause** 1 min, **Reprendre**, filmer 2 min.
2. Cliquer **« Arrêter la vidéo »** : une confirmation s'affiche. Répondre **Non** : rien ne s'arrête. Répondre **Oui** : la captation s'arrête.
3. Cliquer **« Relancer la captation »** et filmer 1 min. Le journal indique « Captation relancée — la numérotation des clips continue ».
4. Dans la galerie : **aucun nom en double**. Les clips vont de clip_0001 à clip_00NN, dans l'ordre.
5. Le chrono et « Go écrits » **cumulent** pause et relance.
6. Terminer la session, en démarrer une nouvelle avec vidéo : le chrono et le disque repartent de **00:00:00** et **0,0 Go**, et la numérotation continue.

**7. Fichiers HD supprimés (V7)**
1. Filmer 2 min sans « pleine qualité », puis terminer la session.
2. Supprimer à la main le dossier `video_…\clips` d'une captation.
3. Rouvrir la galerie : une ligne « … fichier(s) vidéo retiré(s) de la file : introuvable(s) sur le disque (clips …) » est écrite au journal.
4. La carte « Clips HD en attente » ne compte plus ces clips, et « Envoyer les HD maintenant » ne produit aucune erreur.

**8. Dossiers (D2) et journal (D1)**
1. Choisir le même dossier pour les photos et la vidéo : un avertissement orange s'affiche sous le dossier vidéo, et une ligne au journal au démarrage.
2. Après une manche, cliquer **« Exporter le journal »** : le fichier contient la manche entière, lignes d'écran et lignes techniques.
3. Il est aussi exportable depuis l'écran des épreuves (icône à côté de la déconnexion).

**9. Langue**

Passer Windows en anglais, puis parcourir la surveillance vidéo (relance, confirmation d'arrêt) : aucun texte en `clé.technique`.
