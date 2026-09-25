# Agent Terrain 0.3.1 — vitesse et ergonomie

**Date :** 25/09/2026 · **Base :** commit `cd57e13` (0.3.0) · **Périmètre :** agent uniquement. Le dossier `C:\laragon\www\attimo` a seulement été lu.

**Sources :** `DIAGNOSTIC_DEBIT_AGENT.md` et `attimo-agent-debit-envoi-brief.md`. Le fichier `attimo-agent-debit-envoi-brief-2026-09-25.md` n'existe pas dans le dépôt ni sur le Bureau. Seul le brief du 15/09 est présent : je me suis appuyé sur lui et sur ta consigne (« le serveur répond dès la réception, à 3 envois la 4G est utilisée à 100 % »). J'ai vérifié ce point dans le code serveur : `SportUploadController::upload` répond après `receive()`, envoie l'en-tête `Server-Timing` et dédoublonne par empreinte.

**Rien n'a été compilé pour livraison ni déposé.** J'ai seulement lancé `cargo test`, une compilation de test en mode debug dans `src-tauri/target/debug`, sans installeur. Résultat : 94 tests, tous verts, aucun avertissement du compilateur. Le test d'édition passe aussi avec `ATTIMO_EDITION=dev`.

---

## Partie A — Vitesse

### A1. Envois en parallèle : jusqu'à 6, 3 par défaut
- `uploader.rs` : plafond `MAX_PARALLEL = 6` (au lieu de `clamp(1, 3)`). L'écran propose les boutons 1 à 6, et le 3 est actif par défaut (`index.html`, `main.js`).
- **En plus : le réglage change sans relancer la session.** Avant, repasser par « Réglages » et redémarrer créait une nouvelle session locale, et toutes les photos du dossier repartaient. Désormais 6 ouvriers tournent en permanence, et seuls les N premiers travaillent. Revalider les réglages avec le même dossier et le même checkpoint appelle la nouvelle commande `set_parallel`. Le journal affiche « Envois simultanés : 6 (la session continue) ».

### A2. Texte d'aide, 27 langues
Nouveau texte : « 3 convient à la plupart des réseaux. Montez à 6 si le débit le permet ; revenez à 1 seulement si les envois échouent. En 5G, l'envoi est souvent bien plus lent qu'en 4G : privilégiez la 4G seule. » Il est traduit dans les 27 fichiers, en gardant pour chaque langue le vouvoiement ou le tutoiement déjà en place.

### A3. Détection des photos : la cause, et la correction
**Cause.** Il n'y a ni lecture complète du fichier, ni empreinte, ni EXIF. Le frein est ailleurs : chaque fichier attend **500 ms** que sa taille se stabilise (`wait_for_stability`), et les fichiers étaient traités **un par un**, par le scan initial comme par la surveillance en direct. Cela plafonne la détection à 2 photos/s ; tu en as mesuré 1,9. En direct c'était pire encore : Windows envoie une création puis plusieurs modifications pour une même photo, et chaque événement relançait sa propre attente de 500 ms, à la file.

**Correction** (`watcher.rs`). La garantie reste la même : la taille ne doit pas bouger pendant 500 ms.
- Le scan initial vérifie les fichiers par paquets de 16 en parallèle, puis les inscrit dans l'ordre des noms, qui est l'ordre de prise de vue. On passe à plus de 30 photos/s.
- En direct, chaque fichier est vérifié dans sa propre tâche, 16 au plus à la fois. Un registre regroupe les événements répétés d'une même photo. Si un événement arrive pendant qu'une vérification a conclu « encore instable », la photo est revue.
- Les ouvriers d'envoi prennent le fichier suivant dès qu'il arrive. Il n'y a plus d'interrogation toutes les 200 ms.

### A4. Vidéo
- **Plus d'envois empilés.** Le `setInterval` d'une seconde est remplacé par **2 ouvriers** qui attendent chacun la fin de leur envoi. Le backend refuse en plus un troisième envoi simultané (sémaphore dans `process_video_queue`, réponse `busy`), quel que soit le nombre d'appels.
- **Les photos passent d'abord.** Avant chaque morceau de clip et chaque lot d'images, la vidéo attend que les photos en attente soient parties. L'attente est plafonnée à 20 s : sous un flot continu de photos, la vidéo continue d'avancer, au rythme d'une requête toutes les 20 s par ouvrier. Si le photographe met les photos en pause, la vidéo ne leur cède plus rien.
- **Client HTTP partagé** (keep-alive) pour les clips, les images et l'état de session. Avant, chaque envoi ouvrait sa propre connexion TLS.
- **Les clips sont lus morceau par morceau**, par tranches de 5 Mo. Avant, le fichier entier était chargé en mémoire.
- **Images d'analyse groupées par 10.** Le serveur a déjà ce point d'entrée : `POST /api/sport/events/{id}/frames` accepte jusqu'à 10 images (`SportClipFrameController::MAX_FRAMES_PER_REQUEST`). Un lot regroupe toujours des images d'une même session de captation. La réponse détaillée du serveur est lue image par image : seules les images refusées repassent en file. Le délai d'un lot passe à 180 s, parce que le serveur analyse les 10 images avant de répondre.

### A5. Erreurs 429 et 503
- **Photos.** L'agent lit `Retry-After` ; à défaut, il attend 5 s après un 429 et 10 s après un 503, en doublant à chaque répétition, avec un plafond de 300 s. Tous les envois sont suspendus pendant ce délai, et le parallélisme est **divisé par deux**. Il remonte ensuite d'un cran toutes les 30 s sans incident, jusqu'au réglage choisi.
- **Un 429 ne compte jamais comme une tentative** : la photo n'est jamais marquée « échec » pour cette raison. Un 503 répété trois fois, si, car le serveur peut réellement être en panne.
- **Vidéo.** Un 429 ou un 503 remet les éléments en file sans consommer de tentative, et l'ouvrier attend le délai demandé.
- Le journal affiche : « Serveur saturé (429) : pause de 12 s, 3 envoi(s) simultané(s) ».

### A6. Journal honnête
- Le chrono démarre au début de la **tentative réussie**, une fois le fichier lu. Il ne compte donc plus les essais précédents ni les attentes.
- L'agent lit `Server-Timing: app;dur=…` et affiche **« OK en 1,2 s — réseau 0,9 s + serveur 0,3 s »**. Si l'en-tête est absent, il affiche « OK en 1,2 s ».
- La case « Ko/s » devient **« Mb/s envoyés (1 min) »** : les octets réellement envoyés sur la dernière minute, tous envois confondus. C'est le chiffre à comparer à fast.com.

---

## Partie B — Ergonomie

### B1. Pause de la captation vidéo
- Un bouton **« Pause vidéo » / « Reprendre la vidéo »** apparaît dans le panneau vidéo. La session reste ouverte, les photos continuent, et les clips déjà faits continuent de partir. Le chrono s'arrête pendant la pause.
- **Le clip en cours au moment de la pause** est refermé comme à l'arrêt. Le morceau ouvert est rattrapé, avec ses images d'analyse, et ce qui a été filmé depuis le dernier clip complet devient un **clip plus court**, envoyé avec les autres. Rien n'est filmé pendant la pause.
- **« Reprendre »** relance la caméra avec exactement les réglages du premier démarrage, même si l'écran de réglages a changé entre-temps. Côté serveur, la reprise est une **nouvelle session de captation**, numérotée à partir du clip 1. C'est nécessaire parce que les horodatages d'un clip se calculent depuis le début de sa propre captation.
- **Changement de disque :** chaque captation écrit désormais dans **son propre sous-dossier** (`<dossier choisi>\video_<horodatage>\`). Avant, les noms repartaient de zéro à chaque captation (`clip_0001.mp4`, `m_000000.mp4`). Une reprise, ou la course du lendemain dans le même dossier, **écrasait** les fichiers précédents, y compris des clips HD encore en file d'envoi.

### B2. Bouton « Arrêter la vidéo » capricieux : trois causes, trois corrections
1. **Aucun retour visuel pendant un arrêt long.** L'écran attendait la fin de tout le travail avant de bouger : fermeture de FFmpeg, jusqu'à 10 s ; assemblage du dernier clip ; attente d'un assemblage déjà en cours ; puis surtout **la version légère du dernier clip**, un réencodage de plusieurs dizaines de secondes. On croyait le clic ignoré, et on recliquait.
   → Au clic, le bouton affiche aussitôt « Arrêt en cours… » avec une roue, et les deux boutons vidéo se désactivent. La version légère du dernier clip se prépare ensuite en arrière-plan, et le vidage final l'attend.
2. **Double arrêt.** Un second clic, ou « Terminer la session » juste après, lançait un deuxième `stop_recording`. Celui-ci attendait 10 s un signal de FFmpeg déjà consommé, tout en tenant le verrou de la captation, ce qui bloquait le premier arrêt. Puis il rattrapait une seconde fois le même morceau.
   → Côté interface, un seul arrêt à la fois : les appels suivants attendent le même. Côté Rust, un drapeau `arret_demande` fait répondre `ARRET_EN_COURS` à un second arrêt, et `RecordingHandle::stop()` ne réattend plus un FFmpeg déjà parti.
3. **Bouton mort après un arrêt « disque plein ».** La surveillance disque coupait la captation, mais le panneau restait affiché, avec un bouton qui ne faisait plus rien puisqu'il n'y avait plus de captation.
   → L'écran suit désormais l'état réel : captation en cours, en pause, arrêtée ou envoi seul. Les boutons n'apparaissent que s'ils ont un effet.

Au passage : un morceau annoncé en retard par FFmpeg n'est plus rattaché à la captation suivante après une reprise. Le contrôle se fait sur l'identifiant de session.

### B3. Naviguer sans rien couper
- Un bouton **Accueil** (maison) est ajouté sur l'écran de surveillance. « Réglages » existait déjà ; il rouvre maintenant les réglages de la session en cours, même si une autre épreuve a été consultée entre-temps.
- Un **bandeau de session** s'affiche sur tous les écrans sauf la surveillance et la connexion. Il montre l'épreuve, les photos envoyées et en attente, l'état de la captation (en cours ou en pause), les fichiers vidéo à envoyer, et un bouton « Retour à la surveillance ».
- Dans la liste des épreuves, celle qui tourne porte « ● Session en cours ». Un clic dessus ramène à la surveillance, sans rien relancer.
- Revalider les réglages de la même épreuve **complète** la session sans la recommencer : ajouter la vidéo, changer le parallélisme, etc. Démarrer une **autre** épreuve demande confirmation avant de terminer proprement la session en cours. La déconnexion aussi.
- En interne, l'épreuve de la session en cours (`activeEvent`) est désormais distincte de l'épreuve affichée dans les réglages. Sans cette séparation, ouvrir une autre épreuve pendant une captation aurait envoyé les clips suivants vers la mauvaise épreuve.

### B4. Compteurs vidéo
Le panneau vidéo affiche, pour la session en cours (pauses et reprises comprises) :
- **clips filmés** : tous leurs morceaux sont clos ;
- **clips assemblés** ;
- **clips envoyés** : version légère reçue par le serveur ;
- **clips en attente d'envoi** ;
- **images d'analyse**, avec le nombre d'images déjà envoyées.

Les chiffres « envoyés » et « en attente » viennent de la file persistante, restreinte aux captations de la session (nouveau paramètre `sessions` de `video_queue_stats`).

### B5. Envoyer la HD : le parcours simple
- **Dans la surveillance**, un bloc « Vidéo HD » affiche « 4 clip(s) HD à envoyer — 2,3 Go » et « 3 / 7 clips HD envoyés », avec une barre d'avancement. Le bouton **« Envoyer les HD maintenant »** devient « Suspendre l'envoi HD » pendant l'envoi, et un bouton « Relancer les échecs » apparaît s'il y en a.
- **Le soir, sans captation** : quand on ouvre l'épreuve, l'écran de réglages affiche « N clip(s) HD de cette épreuve attendent d'être envoyés (X Go) » et le même bouton. Un clic ouvre une session d'envoi seul, qui ne demande ni dossier ni caméra.
- Un texte d'aide figure aux deux endroits : **« La HD s'envoie depuis l'agent, jamais depuis la galerie : envoyée depuis la galerie, elle crée un doublon. Les photos restent prioritaires. »**
- La case « Envoyer la pleine qualité pendant la course » est conservée. Elle reste synchronisée avec le bouton, et son aide renvoie désormais vers lui.
- Le vidage final (après « Terminer la session ») ne tournait jamais indéfiniment sur des clips HD retenus : il s'arrête quand tout ce qui est autorisé est parti. La HD retenue attend le bouton.

---

## Installation, version, langues

- **C1 — DEV à côté de la production.** `build-preprod.bat` pose `ATTIMO_EDITION=dev` et fusionne `src-tauri/tauri.preprod.conf.json`, qui donne :
  - le nom visible **« Attimo Agent Terrain DEV »**, pour la fenêtre, le menu Démarrer et le dossier d'installation ;
  - l'identifiant `com.attimo-gallery.agent.dev` ;
  - un **code d'installation Windows (upgradeCode) propre**. L'installeur DEV ne remplace donc jamais celui de production, et inversement.

  La **mémoire locale** est séparée : `%APPDATA%\com.attimo-gallery.agent.dev`, pour la base, la file vidéo et le mot de passe. Le dossier est calculé par `build.rs` et vérifié dans le binaire par le script. Un badge orange « v0.3.1 DEV » s'affiche sur les écrans de connexion et d'épreuves.
- **Production.** `build-prod.bat` vide explicitement `ATTIMO_EDITION` et refuse un binaire qui contiendrait la mémoire de DEV. Le nom, l'identifiant, le dossier de données et le code d'installation restent ceux d'aujourd'hui : une mise à jour retrouve ses sessions et son mot de passe. Les URL restent injectées par les scripts.
- **C2 — Version 0.3.1** dans `tauri.conf.json`, `Cargo.toml` et `package.json` (et `package-lock.json`, resté à 0.1.0 jusqu'ici).
- **C3.** 48 nouveaux textes et 2 textes réécrits, dans les 27 langues. Un contrôle automatique vérifie que chaque langue a toutes les clés et les mêmes variables `{…}` que le français. Les 3 lignes d'`en.json` indentées par tabulations sont normalisées en espaces, sans changer leur contenu.
- **Tests ajoutés** : régulation 429/503 (division, délai, remontée, réglage à chaud), lecture de `Retry-After` et de `Server-Timing`, compteur de priorité des photos, réservation des lots d'images (même session, pas de reprise d'un élément réservé, HD retenue), statistiques par session, erreurs de saturation vidéo, images refusées d'un lot, dossier de données de la production.
- **Rien n'a été retiré.** Les anciennes clés de traduction (`upload.success`, `dashboard.speed_unit`) restent dans les fichiers, sans usage.

---

## Ce qui touche le serveur (rien n'a été modifié)

1. **Doublon quand la HD est envoyée depuis la galerie.** C'est un point serveur, non corrigé ici. Cause relevée dans le code :
   - L'envoi depuis la galerie passe par `POST /api/video/{gallery}/chunk` puis `/finalize/{uploadId}` (`app/Modules/Video/routes.php:42-49`, `VideoUploadController`). Le formulaire n'envoie ni `session_id` ni `clip_index` (`Sport/Views/photos/index.blade.php:600-620`).
   - `VideoUploadService::finalizeUpload()` (l. 223-290) fait un `Video::create()` sans rien chercher avant.
   - Or le clip de l'agent existe déjà : `SportClipUploadService::findOrCreateClip()` crée la ligne au passage de la version légère, identifiée par `sport_clip_session_id` et `sport_clip_index`, et la complète en HD via `attachHd()`.
   - Résultat : deux vidéos dans la galerie, et le clip de l'agent reste sans HD, donc non livrable. Aucune empreinte ni contrainte unique ne l'empêche (l'index `idx_videos_sport_clip` n'est pas unique).
   - **Piste :** dans `finalizeUpload`, si la galerie appartient à une épreuve sport, compléter le clip de l'agent resté sans HD (logique d'`attachHd`) au lieu de créer une vidéo. Attention : le nom de fichier seul ne suffit pas à reconnaître le clip, car `clip_0001.mp4` existe dans chaque session de captation. Il faut le chemin complet (`video_<horodatage>\clips\…`), ou une empreinte.
2. **`Server-Timing`** n'est envoyé que par `/api/sport/upload`. Il manque sur `/clips/chunk`, `/clips/finalize` et `/frames` : l'agent sait le lire, mais ne l'affiche aujourd'hui que pour les photos.
3. **Images d'analyse.** `SportClipFrameController::store` analyse chaque image **dans la requête** (dossards, visages) avant de répondre. Avec des lots de 10, une requête dure 10 analyses. Ce serait le même chantier que pour les photos : répondre dès la réception et analyser dans une file. En attendant, l'agent attend jusqu'à 180 s par lot.
4. **Réponse 429.** Le limiteur Laravel (`throttle:api`) renvoie bien `Retry-After`, et l'agent le respecte. Un 503 de maintenance n'en a que si `php artisan down --retry=…` est utilisé.
5. **Rappel du diagnostic** : bcrypt à chaque requête et `pm.max_children` à vérifier avant de généraliser 6 envois par photographe.

---

## Protocole de test pour Fabien (agent DEV, serveur dev-saas)

**0. Installation côte à côte**
1. Fred lance `build-preprod.bat`, puis installe le `.msi` « Attimo Agent Terrain DEV… ».
2. Vérifier que le menu Démarrer contient **deux** agents, et que l'agent de production s'ouvre comme avant : même mot de passe, mêmes épreuves.
3. Ouvrir l'agent DEV : badge orange **« v0.3.1 DEV »**. Il demande son propre mot de passe d'application (dev-saas).

**1. Débit photo** (même lot de 215 photos, 4G seule, fast.com avant chaque manche)
1. Manche 3 envois (défaut), puis manche 6 envois. Noter les durées.
2. Dans le journal, chaque photo affiche « OK en X s — réseau Y s + serveur Z s ». Relever quelques valeurs : le temps serveur devrait rester sous 0,5 s.
3. La case « Mb/s envoyés (1 min) » est à comparer à fast.com.
4. Pendant une manche, repasser par « Réglages » et choisir 6 : le journal affiche « Envois simultanés : 6 (la session continue) », et **aucune** photo déjà envoyée ne repart.

**2. Détection**
Démarrer sur un dossier contenant déjà les 215 photos (« Uploader les fichiers existants ») : « Scan du dossier terminé » doit arriver en moins de 15 s. En direct, déclencher une rafale au boîtier : les photos apparaissent sans retard croissant.

**3. Vidéo pendant les photos**
Captation de 10 min avec des photos en même temps. Les photos gardent leur rythme. Dans le journal, les « Clip N envoyé » arrivent au fil de l'eau, jamais par dizaines d'un coup.

**4. Pause (B1)**
1. Après 3 min, cliquer « Pause vidéo » : le bouton affiche « Mise en pause… », puis le panneau affiche « Captation en pause ». Le journal indique « le clip en cours a été refermé et part avec les autres ».
2. Attendre 1 min : le chrono ne bouge pas, et les clips continuent de partir.
3. Cliquer « Reprendre la vidéo » : le chrono repart d'où il était.
4. Dans le dossier vidéo, vérifier qu'il y a **deux** sous-dossiers `video_…`.

**5. Arrêt (B2)**
Cliquer « Arrêter la vidéo » **une seule fois** : « Arrêt en cours… » s'affiche **immédiatement**, avec la roue. Refaire un essai en double-cliquant, et un autre en cliquant juste après sur « Terminer la session » : pas de blocage de 10 s, pas de clip en double.

**6. Navigation (B3)**
Pendant une captation, cliquer la maison :
1. Le bandeau bleu « Session en cours : … » est visible, et l'épreuve porte « ● Session en cours ».
2. Ouvrir une **autre** épreuve (écran de réglages), attendre 1 min, puis « Retour à la surveillance » : chrono et compteurs ont continué.
3. Tenter « Démarrer » sur l'autre épreuve : une confirmation est demandée.

**7. Compteurs (B4)**
Vérifier la cohérence : filmés ≥ assemblés ≥ envoyés, et « en attente » qui descend.

**8. HD (B5)**
1. Sans cocher « pleine qualité », filmer 5 min puis arrêter. Le bloc « Vidéo HD » annonce « N clip(s) HD à envoyer — X Go ».
2. « Envoyer les HD maintenant » : la barre avance. « Suspendre l'envoi HD » arrête après le clip en cours.
3. Terminer la session, fermer l'agent, le rouvrir, choisir l'épreuve : la carte « Clips HD en attente » est visible. « Envoyer les HD maintenant » ouvre la surveillance en envoi seul, jusqu'à « Tous les clips HD sont envoyés ».
4. Vérifier dans la galerie : **une seule** vidéo par clip.

**9. Langue**
Passer Windows en anglais (ou une autre langue), relancer l'agent DEV et parcourir les écrans : aucun texte en `clé.technique`.
