# Agent Terrain 0.3.5 — dernier lot de l'agent terrain

**Date :** 30/09/2026 · **Base :** commit `204f453` (0.3.4) · **Serveur :** dépôt Attimo au commit `0e0e986a`, lu et pas modifié. **Aucun point ne demande de changement serveur.**

**Tests :** 149 tests Rust (145 avant, 4 nouveaux) et 16 tests d'interface (Node, 12 avant). Tous passent, sans avertissement du compilateur. Le contrôle des traductions est vert (27 langues).

**Installateurs, non signés, déposés nulle part :**

| Édition | Fichier | Vérification du script |
|---|---|---|
| **DEV** (`build-preprod.bat`) | `src-tauri\target\release\bundle\msi\Attimo Agent Terrain DEV_0.3.5_x64_en-US.msi` (NSIS à côté : `bundle\nsis\Attimo Agent Terrain DEV_0.3.5_x64-setup.exe`) | « OK : binaire de preprod valide » : URL `dev-saas` présente, mémoire `com.attimo-gallery.agent.dev` présente |
| **PRODUCTION** (`build-prod.bat`) | `src-tauri\target\release\bundle\msi\Attimo Agent Terrain_0.3.5_x64_en-US.msi` (NSIS à côté : `bundle\nsis\Attimo Agent Terrain_0.3.5_x64-setup.exe`) | « OK : binaire de production valide » : aucune trace de `dev-saas` ni de la mémoire DEV, URL `app.attimo-gallery.com` présente. Contrôle refait à la main sur le binaire final : mêmes résultats. |

Pour la production, le script rappelle de renommer le `.msi` en `Attimo_Agent_Terrain_international.msi` avant le dépôt. Je ne l'ai pas fait, et rien n'a été déposé.

**État de départ :** aucun fichier suivi n'était modifié. Les deux fichiers non suivis déjà signalés en 0.3.4, `DIAGNOSTIC_DEBIT_AGENT.md` et `attimo-agent-debit-envoi-brief.md`, sont toujours là. Je n'y ai pas touché et ils ne sont pas dans le commit.

> **⚠ Compte rendu de Fabien introuvable.** `attimo-agent-034-compte-rendu-pour-fred-2026-09-30.md` n'est ni à la racine, ni dans le dépôt serveur, ni ailleurs dans le dossier utilisateur. Les points 1 à 4 sont corrigés d'après la demande, qui les décrit en entier. **Le point 5 (« tout autre point marqué à corriger ») n'a pas pu être traité** : déposer le fichier à la racine pour un complément.

---

## 1. Le message d'arrêt ne compte que les clips de cette captation

**Cause.** « Captation arrêtée — N clip(s) produit(s) » additionnait les clips de **toutes** les captations ouvertes depuis le début de la session. Après un « Relancer la captation », le deuxième arrêt reprenait donc aussi les clips du premier.

**Correction (`main.js`).**
- L'agent retient le point de départ de la captation en cours : le dernier « Démarrer » ou « Relancer ».
- Le message ne compte que les clips produits depuis ce point (`videoTotalCaptation`).
- Une pause suivie d'une reprise reste dans la même captation, et ses clips comptent.
- Le texte le dit désormais : « Captation arrêtée — 3 clip(s) produit(s) **par cette captation** ».
- Les compteurs du bloc vidéo (« clips filmés », « clips assemblés ») gardent le total de la session, comme avant.

## 2. « Envoyées » et « déjà en ligne » ne se mélangent plus

**Cause.** Pour l'agent, une photo « envoyée » était une photo reçue par le serveur, y compris celles qu'il avait déjà. D'où « Terminé : 25 photos envoyées avec succès ! (dont 25 déjà en ligne, non renvoyées) », alors qu'aucune n'était partie.

**Correction (`affichage.js`, `main.js`).**

| Où | Avant | Après |
|---|---|---|
| Bilan de fin, rien d'envoyé | « 25 photos envoyées avec succès ! (dont 25 déjà en ligne…) » | « Terminé : aucune photo à envoyer, les 25 sont déjà en ligne (non renvoyées) » |
| Bilan de fin, les deux cas | « 25 envoyées (dont 5 déjà en ligne…) » | « Terminé : 20 photo(s) envoyée(s), 5 déjà en ligne (non renvoyées) » |
| Bilan de fin, avec des échecs | « 25 envoyées, 2 en échec (dont 5…) » | « Terminé : 20 envoyée(s), 5 déjà en ligne (non renvoyées), 2 en échec » |
| Tuile « Envoyées » | 25 | **20**, avec dessous « + 5 déjà en ligne » |
| Bandeau de session | « 25 photo(s) envoyée(s), 0 en attente » | « 20 photo(s) envoyée(s), 5 déjà en ligne, 0 en attente » |

Sans photo déjà en ligne, les libellés sont inchangés. La barre de progression avance toujours sur toutes les photos traitées.

## 3. Le journal s'exporte dans un dossier de l'agent

**Cause.** « Exporter le journal » ouvrait une boîte « Enregistrer sous ». Windows l'ouvre sur le dernier dossier choisi, qui était en général le dossier des photos. Le journal atterrissait donc dans un dossier surveillé.

**Correction (`journal.rs`, `commands.rs`, `main.js`).**
- Plus de boîte de dialogue. Le fichier va toujours dans **`Documents\Attimo Agent Terrain\Journaux`**, sous le nom `attimo-agent-journal-AAAAMMJJ-HHMMSS.log`.
- **Jamais dans un dossier surveillé.** Si ce dossier est, ou se trouve dans, le dossier photos ou le dossier vidéo (par exemple quand « Documents » lui-même est surveillé), l'export va dans le dossier de données de l'agent (`%APPDATA%\com.attimo-gallery.agent[.dev]\exports`). C'est aussi le cas si « Documents » n'est pas accessible en écriture.
- **Où, dans l'interface :**
  - l'infobulle des deux boutons « Exporter le journal » donne le dossier ;
  - après l'export, une ligne de journal et une fenêtre donnent le chemin complet du fichier ;
  - l'Explorateur de fichiers s'ouvre, avec le fichier sélectionné.
- L'agent n'accepte qu'un nom de fichier, pas un chemin.

## 4. Épreuve en brouillon : dit clairement, et l'envoi repart seul

**Ce que fait le serveur** (lu dans le dépôt, sans rien modifier).
- `GET /api/sport/events` renvoie les brouillons, avec `is_published: false`.
- Une épreuve jamais publiée n'a pas de galerie. Tant que c'est le cas, les routes des clips et des images d'analyse répondent **422 « Event has no gallery. »**.
- La galerie est créée à la publication. Elle l'est aussi quand le photographe ouvre la page de l'épreuve sur le site, même si l'épreuve reste en brouillon.
- Les photos sont acceptées en brouillon, et visibles une fois l'épreuve publiée.

**Cause.** En 0.3.4, ce 422 était pris pour un refus définitif : le clip sortait de la file « en échec », sans rien expliquer. L'écran montrait « 0 clips envoyés / 3 en attente ».

**Correction (`video_uploader.rs`, `commands.rs`, `auth.rs`, `main.js`).**
- **Liste des épreuves :** un badge « Brouillon » s'affiche sur chaque épreuve non publiée.
- **Tableau de bord :** un avis en haut de l'écran : « Épreuve en brouillon : publiez-la sur Attimo. Tant qu'elle ne l'est pas, rien n'est visible dans la galerie et les clips vidéo peuvent rester en attente dans l'agent. »
- **Refus du serveur :** le 422 « Event has no gallery. » n'est plus un échec.
  - Les clips et les images restent en file, sans consommer d'essai.
  - Le journal et le bloc vidéo le disent une fois : « Clips en attente : l'épreuve est en brouillon. Publiez-la sur Attimo, l'envoi partira de lui-même. »
  - La file de cette épreuve n'est redemandée qu'une fois par minute.
- **Publication :** tant que l'avis est affiché, l'agent demande chaque minute au serveur si l'épreuve a été publiée (`GET /api/sport/events/{id}/status`, route existante). Dès qu'elle l'est :
  - l'avis disparaît ;
  - le journal écrit « Épreuve publiée : les envois en attente repartent. » ;
  - la file vidéo repart aussitôt.
- Les clips mis « en échec » par une 0.3.4 sur un brouillon ne repartent pas seuls : il faut cliquer « Relancer les échecs ».

---

## Ce qui a changé, fichier par fichier

| Fichier | Changement |
|---|---|
| `main.js` | Compteur par captation, tuile, bandeau et bilan des photos, export du journal, brouillon (badge, avis, attente de la file, vérification de publication). |
| `affichage.js` | `bilanPhotos`, `comptePhotos`. |
| `index.html`, `styles.css` | Note « + N déjà en ligne », avis de brouillon, badge « Brouillon », infobulle d'export. |
| `journal.rs` | `dossier_export` (jamais dans un dossier surveillé), `nom_export_valide`. |
| `commands.rs` | `export_journal(nom, surveilles)` écrit dans le dossier de l'agent et ouvre l'Explorateur ; `event_published` ; refus « brouillon » remis en file sans essai. |
| `video_uploader.rs` | `EPREUVE_BROUILLON`, reconnu au message du serveur, sur les morceaux, la finalisation et les images. |
| `auth.rs` | `is_published` lu dans la liste des épreuves ; `fetch_event_published`. |
| `lib.rs` | Commande `event_published` déclarée. |
| `src/lang/*.json` | Dans les 27 langues, 11 textes nouveaux et 1 modifié (`video_stopped`). `complete.already` et `journal.export_dialog` sont retirés, car ils ne servent plus. |
| Version | 0.3.5 dans `tauri.conf.json`, `Cargo.toml`, `Cargo.lock`, `package.json` et `package-lock.json`. |

**Tests ajoutés.**
- Dossier d'export : Documents, « Documents » surveillé (casse indifférente), dossier de l'agent surveillé, dossier voisin au nom proche, pas de Documents.
- Nom d'export : un chemin est refusé.
- Refus « Event has no gallery. » : reconnu comme brouillon et non comme échec définitif. Les autres 422 ne sont pas confondus avec lui.
- `is_published` lu dans la liste et dans la réponse de statut.
- Bilan des photos (4 cas) et tuile « Envoyées ».

**Non vérifié ici.** Aucun essai avec une caméra ni sur le serveur de dev. Le point 4 repose sur la lecture du code serveur. La recette ci-dessous le confirmera.

---

## Recette pour Fabien — agent DEV 0.3.5, sur dev

1. **Installation.** Installer le `.msi` DEV : le badge affiche « v0.3.5 DEV » et la connexion est conservée.
2. **Message d'arrêt (point 1).**
   1. Filmer 3 min, puis « Arrêter la vidéo ». Le message dit « … N clip(s) produit(s) par cette captation », et N est égal au nombre de lignes « Clip N assemblé ».
   2. « Relancer la captation », filmer 3 min, puis arrêter. Le message ne compte que les clips de cette deuxième captation.
   3. Refaire le test avec une pause suivie d'une reprise avant l'arrêt. Les clips d'avant la pause comptent.
3. **Photos déjà en ligne (point 2).**
   1. Relancer une session sur un dossier déjà envoyé.
   2. La tuile « Envoyées » affiche 0, avec dessous « + 25 déjà en ligne ».
   3. La fin dit « Terminé : aucune photo à envoyer, les 25 sont déjà en ligne (non renvoyées) ».
   4. Ajouter 3 nouvelles photos : « 3 photo(s) envoyée(s), 25 déjà en ligne ».
4. **Journal (point 3).**
   1. Survoler « Exporter le journal » : l'infobulle donne le dossier.
   2. Cliquer : aucune boîte « Enregistrer sous ». Une fenêtre donne le chemin, `Documents\Attimo Agent Terrain\Journaux\…`, et l'Explorateur s'ouvre sur le fichier.
   3. Le dossier des photos ne contient aucun `.log`.
   4. Variante : prendre « Documents » comme dossier photos, puis exporter. Le fichier va sous `%APPDATA%\com.attimo-gallery.agent.dev\exports`, et rien ne part vers la galerie.
5. **Brouillon (point 4).**
   1. Créer une épreuve sur le site **sans la publier et sans ouvrir sa page photos**. Dans l'agent, elle porte le badge « Brouillon ».
   2. L'ouvrir et filmer 3 min. L'avis de brouillon est en haut. Le journal et le bloc vidéo disent une fois « Clips en attente : l'épreuve est en brouillon… ». Aucun clip n'est « en échec ».
   3. Publier l'épreuve sur le site. En moins d'une minute, l'avis disparaît, le journal écrit « Épreuve publiée : les envois en attente repartent. » et les clips partent.
6. **Non-régression, rapide.** Les 8 tests de la 0.3.4, en particulier :
   - B3 : « Déjà en ligne » pour tout ;
   - B4 : la numérotation continue ;
   - B6 : bloc HD « 5 / 5 » ;
   - la reprise revérifie la galerie ;
   - la tuile Mb/s affiche une valeur pendant l'envoi vidéo.
