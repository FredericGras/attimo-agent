# Agent Terrain 0.3.4 — corrections de la recette de Fabien

**Date :** 29/09/2026 · **Base :** commit `7ffcbcd` (0.3.3) · **Serveur :** dépôt Attimo au commit `a1ebc52`, lu et pas modifié. **Aucun point ne demande de changement serveur.**

**Sources :** `attimo-compte-rendu-0-3-3-ocr-bis-2026-09-29.md` (recette de Fabien) et `LOT_SPORT_SERVEUR_29_09.md` §7 et §9 (lecture vidéo sans clip, n° 432), dans le dépôt serveur.

**Tests :** 145 tests Rust (132 avant, 13 nouveaux) et 12 tests d'interface (Node). Tous passent, sans avertissement du compilateur. Le contrôle des traductions est vert.

**Installateur DEV :** `src-tauri\target\release\bundle\msi\Attimo Agent Terrain DEV_0.3.4_x64_en-US.msi`. Un installateur NSIS est produit à côté : `src-tauri\target\release\bundle\nsis\Attimo Agent Terrain DEV_0.3.4_x64-setup.exe`. Il a été compilé par `build-preprod.bat`, qui a aussi vérifié le binaire (URL de préproduction et mémoire séparée). L'édition production n'a pas été compilée. Rien n'est signé et rien n'a été déposé sur le serveur.

**État de départ :** aucun fichier suivi n'était modifié. Deux fichiers non suivis, `DIAGNOSTIC_DEBIT_AGENT.md` et `attimo-agent-debit-envoi-brief.md`, dataient d'avant ce lot. Je n'y ai pas touché et ils ne sont pas dans le commit.

---

## 1. « Captation arrêtée — N clip(s) produit(s) » compte un clip de moins

**Cause.** L'arrêt écrit son message dès que FFmpeg a rendu la main et que le dernier clip est assemblé. Le compteur, lui, n'était augmenté qu'au moment où le clip entrait en file (`mettreClipEnFile`). Or, pour les clips refermés à l'arrêt, cette mise en file ne venait qu'**après** celle de leurs images d'analyse, qui prend quelques instants. Le message partait donc avant que le dernier clip soit compté : 4 pour 5, 7 pour 8, 2 pour 3.

**Correction (`main.js`).** Les clips rendus par l'arrêt sont comptés tout de suite, avant toute attente (`compterClipAssemble`). Ils sont ensuite mis en file sans être comptés une deuxième fois.

## 2. Images d'analyse sans clip à la fin d'une captation

**Cause.** Trois défauts s'additionnaient.

1. **Deux ancres de temps différentes.** Les images étaient datées depuis le départ de la captation retenu par l'agent (`started_at`, après le lancement de FFmpeg). Les clips, eux, étaient datés depuis l'heure du clic, prise par l'interface un peu plus tôt. Les clips tombaient donc un peu **avant** les images. Pour un clip du milieu, le chevauchement du clip suivant rattrape l'écart. Pour le dernier clip, rien ne le rattrape : ses dernières images tombaient après sa fin.
2. **Un arrondi à la seconde.** La durée mesurée du dernier clip était arrondie à la seconde (`round`), ce qui pouvait retirer jusqu'à une demi-seconde à sa fin.
3. **Des images au-delà de la fin réelle.** Une image est datée d'après son rang dans le morceau (rang × 2 s). FFmpeg peut sortir une dernière image au bord du morceau, donc au-delà de la fin mesurée du clip.

S'y ajoutait une **course à l'arrêt**, qui pouvait faire perdre le dernier clip entier. Quand un morceau se ferme, l'interface commence par en extraire les images (quelques secondes), et seulement ensuite demande l'assemblage. Si l'arrêt tombait pendant ce temps, le dernier morceau arrivait au suivi sans son prédécesseur. Le clip qui les réunit n'était alors jamais assemblé, et les images de ces instants n'avaient plus de clip.

Côté serveur, ces images restaient « En attente de son clip » (n° 432).

**Correction.**
- **Même ancre pour tout** (`assembler.rs`). L'agent date lui-même chaque clip (`started_at`, `ended_at`) sur le départ qui date les images. L'interface envoie ces deux valeurs telles quelles. Le manifeste utilise les mêmes.
- **Durée à la milliseconde** (`commands.rs::mesurer_duree`). La fin du dernier clip est sa durée mesurée, sans arrondi. Un écart de moins d'une demi-seconde avec la durée prévue ne rend plus le clip « incomplet ».
- **Aucune image au-delà de la fin d'un clip** (`images_couvertes`). À l'arrêt, une image du dernier morceau n'est gardée que si un clip la couvre, avec la même règle que le serveur : `début <= instant < fin`. Les autres ne partent pas et n'entrent pas à l'index du manifeste. Le journal technique écrit alors « Dernier morceau : N image(s) d'analyse après la fin du dernier clip, non envoyée(s) ». Les fichiers restent sur le disque, conformément au contrat.
- **Le dernier clip est toujours assemblé** (`rattraper_morceaux_clos`). Avant de traiter le dernier morceau, l'arrêt déclare lui-même, dans l'ordre, les morceaux déjà fermés que l'interface n'a pas encore fait assembler. Si l'interface le fait plus tard, elle ne trouve plus rien à faire, et il n'y a donc jamais de doublon.

## 3. Bloc HD

**Barre et compteur.**
- *Cause :* le total comptait les clips introuvables : « 5 / 13 » (38 %), une fois tout envoyé.
- *Correction :* la barre et le compteur portent sur les clips envoyables, c'est-à-dire à envoyer, envoyés ou en échec. Le bloc affiche donc « 5 / 5 ». Les introuvables sont annoncés à part : « · 8 introuvable(s) sur le disque » pendant l'envoi, puis « 5 envoyés — 8 introuvables… » à la fin.

**Débit et temps restant, en envoi manuel comme automatique.**
- *Cause :* le débit était calculé dans l'interface, à partir des octets HD relevés toutes les 2 s sur une minute. Il divisait donc aussi les temps morts : attente des photos, assemblage, finalisation, pauses entre deux clips en HD automatique. D'où « 0,3 Mb/s » sur une 4G à 20 Mb/s. Il fallait aussi au moins 8 s de relevés avant d'afficher quoi que ce soit.
- *Pourquoi rien ne s'affichait en envoi manuel :* je n'ai pas pu le reproduire sans matériel. La seule explication trouvée dans le code est ce seuil de 8 s combiné à des clips courts. La nouvelle mesure ne dépend plus de ce calcul : c'est ce point de la recette qui confirmera.
- *Correction (`video_uploader.rs`) :* l'agent chronomètre chaque morceau HD, de l'envoi de la requête jusqu'à la réponse du serveur, sans l'attente des photos. Le débit est le nombre d'octets divisé par le temps où au moins un morceau était réellement en route. Deux envois simultanés ne comptent qu'une fois leur temps commun. Il porte sur les 12 derniers morceaux, soit environ 60 Mo, pris dans les 10 dernières minutes. Il est disponible dès le premier morceau envoyé, en envoi manuel comme automatique. Le temps restant en découle.

**Bilan de la session précédente.**
- *Cause :* le bloc comptait les clips envoyés et introuvables de toute l'épreuve, sans distinction de session.
- *Correction :* envoyés et introuvables ne comptent que ce qui est sorti de la file depuis l'ouverture de la session. Un clip introuvable est désormais daté au moment où il sort de la file. Les clips retirés à l'ouverture de l'épreuve, juste avant la session, en font partie : c'est le cas B6. Ce qui reste à envoyer, ou en échec, compte toujours pour toute l'épreuve, puisque c'est encore à faire. Au démarrage d'une nouvelle session, le bloc affiche donc « Aucun clip HD pour l'instant. » au lieu de l'ancien bilan.

## 4. Tuile « Mb/s envoyés (1 min) »

**Cause.** Seuls les envois de photos y entraient.

**Correction (`main.js::releverOctetsVideo`).** Toutes les 2 s, les octets vidéo envoyés depuis le relevé précédent (clips légers, HD, images d'analyse) s'ajoutent à la même fenêtre d'une minute que les photos. C'est aussi vrai pendant le vidage final, après la fermeture de la session.

## 5. « Reprendre » revérifie la galerie

**Cause.** Le hash-check n'était fait qu'au scan de démarrage. « Reprendre » ne faisait que relancer les envois.

**Correction.**
- `resume_session` lance en tâche de fond `watcher::reverifier_galerie`. L'envoi reprend aussitôt, sans attendre la vérification.
- Seules les photos **reçues par le serveur** pendant cette session sont vérifiées (statut envoyée ou déjà en ligne). Leur empreinte est désormais notée à l'envoi, dans une nouvelle colonne `upload_files.empreinte` ajoutée automatiquement à la base existante. Il n'y a donc pas de fichier à relire, sauf pour les photos envoyées avant la 0.3.4.
- Une photo que le serveur n'a plus est remise en attente et repart.
- Le journal écrit « Galerie revérifiée : N photo(s) absente(s), renvoi en cours », ou « … les N photo(s) envoyée(s) sont en ligne ».
- Comme au démarrage, la route est redemandée au serveur. S'il ne répond pas, rien ne change et une ligne est écrite au journal technique.
- Deux « Reprendre » rapprochés ne lancent pas deux vérifications en même temps.

## 6. « Renvoyée » réservé à une photo déjà envoyée

**Cause.** Au scan, l'avis du serveur est demandé par groupes de 64 photos, **avant** leur envoi. Si la même photo partait entre-temps, par exemple parce qu'une copie du fichier se trouve dans le dossier, la mémoire locale la connaissait au moment de l'envoi. L'ancien avis « absente » la faisait alors dire « Absente de la galerie — renvoyée », alors que ce fichier n'était jamais parti.

**Correction (`uploader.rs`).**
- « Renvoyée » exige deux conditions : la mémoire de l'agent sait la photo en ligne (envoyée, ou trouvée en ligne), **et** le serveur, interrogé pour elle seule au moment de l'envoi, la dit absente.
- Si le serveur la voit, la photo est « Déjà en ligne — non renvoyée ».
- Si le serveur ne répond pas, l'avis du scan reste valable.
- À la reprise, une photo en attente, en cours ou en échec n'est jamais candidate au renvoi : elle n'a jamais été envoyée.

---

## Ce qui a changé, fichier par fichier

| Fichier | Changement |
|---|---|
| `assembler.rs` | Le clip porte `duree_ms`, `started_at` et `ended_at`, datés sur le départ de la captation (`AssembledClip::dater`). Le suivi connaît ce départ et le premier morceau attendu. |
| `commands.rs` | Durée mesurée à la milliseconde ; morceaux fermés rattrapés à l'arrêt ; images du dernier morceau filtrées puis inscrites au manifeste ; `resume_session` revérifie la galerie ; `video_queue_stats` accepte `hd_since_ms` ; `video_bytes_sent` renvoie `hd_mbps`. |
| `frames.rs`, `manifest.rs` | Instant de l'image en millisecondes (interne) ; fin des clips à la milliseconde dans le manifeste. |
| `video_uploader.rs` | Chronométrage des morceaux HD, `debit_actif`. |
| `video_queue.rs` | `bilan_hd_depuis` ; un clip introuvable est daté à sa sortie de la file. |
| `database.rs`, `uploader.rs`, `watcher.rs`, `lib.rs` | Colonne `empreinte`, photos reçues, remise en attente, revérification à la reprise, règle « renvoyée ». |
| `main.js`, `affichage.js` | Comptage à l'arrêt, horodatages de l'agent, bloc HD, tuile Mb/s, ligne de journal de la reprise. |
| `src/lang/*.json` | 3 nouveaux textes dans les 27 langues : `upload.gallery_recheck_missing`, `upload.gallery_recheck_ok`, `hd.missing_apart`. |
| Version | 0.3.4 dans `tauri.conf.json`, `Cargo.toml`, `Cargo.lock`, `package.json` et `package-lock.json`. |

**Tests ajoutés.**
- Datation d'un clip et fin à la milliseconde.
- Images au-delà de la fin du dernier clip, borne haute exclue comme sur le serveur.
- Débit HD : temps morts exclus, envois simultanés.
- Bilan HD limité à la session.
- Photos reçues seules candidates à la revérification, remise en attente, tri des absentes.
- Bloc HD : cas B6 « 5 / 5 », nouvelle session sans bilan, envoi manuel avec débit.

Le test JS de l'ancien calcul de débit est supprimé, avec ce calcul.

**Non vérifié ici.** Aucun essai avec une caméra ni sur le serveur de dev. Le point 2 repose sur la lecture du code des deux côtés, et le point 3 « débit en envoi manuel » sur une mesure réécrite. Les deux se confirment à la recette ci-dessous.

---

## Recette pour Fabien — agent DEV 0.3.4, sur dev, dans des événements neufs

1. **Installation.**
   - Installer le `.msi` ci-dessus : le badge affiche « v0.3.4 DEV » et la connexion est conservée.
2. **Message d'arrêt (point 1).**
   - Filmer environ 3 min, puis « Arrêter la vidéo ».
   - Le nombre du message « Captation arrêtée — N clip(s) produit(s) » est égal au nombre de lignes « Clip N assemblé » et de fichiers `clip_*.mp4` du dossier `clips`.
   - Refaire le test avec une pause, puis une reprise, avant l'arrêt.
3. **Dernier clip et images (point 2).** Réglage par défaut (clip de 150 s).
   1. Montrer un dossard dans les **5 dernières secondes**, puis arrêter. Page Identifications : la lecture a un clip, et aucune ligne « En attente de son clip ».
   2. Relancer, puis arrêter **entre 2:31 et 2:34** au chrono, juste après la fin du premier clip. Le message annonce 2 clips, et les deux sont en ligne.
   3. Requête D du lot serveur (`video_id IS NULL`) sur ces événements : aucune ligne.
4. **Bloc HD (point 3), protocole B6.**
   1. Filmer 3 min sans pleine qualité, puis terminer la session.
   2. Supprimer le dossier `clips` d'une captation.
   3. Rouvrir l'épreuve, puis cliquer « Envoyer les HD maintenant ».
   4. Pendant l'envoi : « N / M clips HD envoyés · X Mb/s · reste ≈ … · K introuvable(s) sur le disque ». M ne compte que les clips envoyables, et le débit doit être proche de celui de la 4G.
   5. À la fin : « M / M » avec la barre pleine, et « M envoyés — K introuvables… ».
   6. Ouvrir ensuite une nouvelle session sur la même épreuve : « Aucun clip HD pour l'instant. », sans l'ancien bilan.
5. **HD automatique.**
   - Cocher la pleine qualité et filmer 2 min. Le débit affiché est celui de la 4G, et non plus « 0,3 Mb/s ».
6. **Tuile Mb/s (point 4).**
   - Sans aucune photo, pendant l'envoi des clips, la tuile « Mb/s envoyés (1 min) » affiche une valeur au lieu de « — ».
7. **Reprise (points 5 et 6).**
   1. Session photo, envoyer une vingtaine de photos, puis « Pause ».
   2. Supprimer 2 photos de la galerie, et déposer 3 **nouvelles** photos dans le dossier.
   3. « Reprendre ». Le journal affiche « Galerie revérifiée : 2 photo(s) absente(s), renvoi en cours ». Seules ces 2 photos sont dites « Absente de la galerie — renvoyée ». Les 3 nouvelles partent normalement, sans « renvoyée ». Aucun doublon dans la galerie.
   4. « Pause » puis « Reprendre » sans rien supprimer : « Galerie revérifiée : les N photo(s) envoyée(s) sont en ligne », et rien ne repart.
8. **Non-régression, rapide.**
   - B3 : relancer une session sur un dossier déjà envoyé donne « Déjà en ligne » pour tout, et « renvoyée » pour les seules photos supprimées.
   - B4 : la numérotation continue à travers pause, arrêt et relance.
   - B5 : la mesure des envois photo est inchangée.
   - B8 : le journal donne le dossier de chaque captation.
   - B9 : le manifeste « Aucune » est inchangé.
