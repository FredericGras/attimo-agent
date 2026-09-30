# Agent Terrain 0.3.6 — tuile des images d'analyse et avis de brouillon

**Date :** 30/09/2026 · **Base :** commit `cfa0102` (0.3.5) · **Serveur :** dépôt Attimo lu, pas modifié. **Aucun changement serveur n'est demandé.**

**Source :** le point « Brouillon » du compte rendu de Fabien (`Téléchargements\attimo-lot-final-agent035-compte-rendu-pour-fred-2026-09-30.md`) : « après la publication, la tuile de l'agent reste à « 21 images d'analyse — 0 envoyées », alors que le serveur a produit les lectures. L'avis jaune met aussi quelques secondes à disparaître. »

**Tests :** 152 tests Rust (149 avant, 3 nouveaux) et 18 tests d'interface (Node, 16 avant). Tous passent, sans avertissement du compilateur. Le contrôle des traductions est vert (27 langues).

**Installateurs, non signés, déposés nulle part :**

| Édition | Fichier | Vérification du script |
|---|---|---|
| **DEV** (`build-preprod.bat`) | `src-tauri\target\release\bundle\msi\Attimo Agent Terrain DEV_0.3.6_x64_en-US.msi` (NSIS à côté : `bundle\nsis\Attimo Agent Terrain DEV_0.3.6_x64-setup.exe`) | « OK : binaire de preprod valide » : URL `dev-saas` présente, mémoire `com.attimo-gallery.agent.dev` présente |
| **PRODUCTION** (`build-prod.bat`) | `src-tauri\target\release\bundle\msi\Attimo Agent Terrain_0.3.6_x64_en-US.msi` (NSIS à côté : `bundle\nsis\Attimo Agent Terrain_0.3.6_x64-setup.exe`) | « OK : binaire de production valide ». Contrôle refait à la main sur le binaire final : aucune trace de `dev-saas` ni de la mémoire DEV, URL `app.attimo-gallery.com` présente. |

Pour la production, le script rappelle de renommer le `.msi` en `Attimo_Agent_Terrain_international.msi` avant le dépôt. Je ne l'ai pas fait, et rien n'a été déposé.

**État de départ :** aucun fichier suivi n'était modifié. Les deux fichiers non suivis déjà signalés, `DIAGNOSTIC_DEBIT_AGENT.md` et `attimo-agent-debit-envoi-brief.md`, sont toujours là. Je n'y ai pas touché et ils ne sont pas dans le commit.

---

## Causes

Trouvées en lisant le code de l'agent et du serveur. Je n'ai pas reproduit le problème sur le serveur de dev.

1. **La tuile ne comptait que les images confirmées.** Les images d'analyse partent par lots de 10, en une requête. Le serveur analyse les 10 (dossards, visages) et répond ensuite. Il enregistre chaque lecture dès qu'elle est faite, mais l'agent ne compte le lot comme « envoyé » qu'après la réponse. Entre les deux, parfois plusieurs dizaines de secondes, le serveur avait déjà produit des lectures et la tuile affichait encore « 0 envoyées ». Les images en cours d'envoi n'étaient pas affichées.
2. **Les images abandonnées étaient invisibles.** Si le serveur refuse une image trois fois (par exemple quand la reconnaissance AWS est saturée), l'image passe « en échec ». Elle ne figurait ni dans la tuile ni dans le journal, et la tuile pouvait rester à 0 pour de bon.
3. **Le cas « brouillon » de la 0.3.5 n'était jamais atteint.** Avant d'envoyer quoi que ce soit, l'agent demande au serveur ce qu'il a déjà reçu (`clips/status`). Sur une épreuve en brouillon, le serveur répond « Event has no gallery. ». L'agent lisait cette réponse comme illisible et réessayait toutes les 3 s. Du coup, la ligne « Clips en attente : l'épreuve est en brouillon… » n'apparaissait pas, et l'attente d'une minute par épreuve ne s'appliquait pas.
4. **L'avis jaune était vérifié une fois par minute.** Les envois repartaient en 3 s, mais l'avis restait jusqu'à la vérification suivante, soit jusqu'à une minute.

## Corrections

**Tuile « images d'analyse » (`affichage.js`, `main.js`, `video_queue.rs`)**
- Elle dit ce qui part, dès le départ : « 21 images d'analyse — 0 envoyées, **10 en cours d'envoi** ».
- Elle passe à « 10 envoyées » dès que le serveur a répondu pour le lot.
- Elle affiche les abandons : « … 18 envoyées, **3 en échec** ».
- Le journal le dit aussi : « 3 image(s) d'analyse abandonnée(s) après 3 essais ». « Relancer les échecs » les renvoie, comme avant.
- Le libellé est construit par une fonction testée (`Affichage.libelleImagesAnalyse`). La file compte à part les images en échec (`frames_failed`).

**Brouillon reconnu dès la première question (`video_uploader.rs`, `commands.rs`)**
- Le refus « Event has no gallery. » de `clips/status` est maintenant reconnu comme un brouillon. On retrouve donc le comportement prévu en 0.3.5 :
  - une ligne au journal ;
  - l'avis dans le bloc vidéo ;
  - les fichiers restent en file sans consommer d'essai ;
  - la file de l'épreuve n'est redemandée qu'une fois par minute, au lieu de deux appels toutes les 3 s.

**Avis jaune (`main.js`)**
- Tant qu'il est affiché, la publication est vérifiée **toutes les 10 s** (au lieu de 60).
- Dès qu'elle est constatée, trois choses se passent ensemble :
  - l'avis disparaît ;
  - le journal écrit « Épreuve publiée : les envois en attente repartent. » ;
  - la file repart aussitôt.
- Si un envoi réussit pendant que l'avis est affiché, la publication est vérifiée tout de suite, au plus une fois toutes les 5 s. Si l'épreuve est encore en brouillon, l'avis reste. C'est le cas quand sa page a été ouverte sur le site, parce que la galerie existe alors déjà.
- Délai entre la publication et les premiers envois : 10 s au plus (avant : 3 s environ). L'avis et les envois changent désormais en même temps.

**Rien d'autre ne change.** Les photos, les clips, le bloc HD, les compteurs de la 0.3.5 et l'export du journal restent tels quels. Les épreuves publiées ne sont pas concernées par les points 3 et 4.

## Fichiers

| Fichier | Changement |
|---|---|
| `video_uploader.rs` | `etat_session` reconnaît le brouillon (`etat_session_sur`, testable) ; 2 tests avec un serveur factice. |
| `commands.rs` | Brouillon sur `clips/status` → réponse `unpublished`. |
| `video_queue.rs` | `frames_failed` dans le décompte ; 1 test. |
| `affichage.js` | `libelleImagesAnalyse` ; 2 tests dans `tests/affichage.test.cjs`. |
| `main.js` | Tuile, ligne de journal des images abandonnées, vérification de publication toutes les 10 s et après un envoi réussi. |
| `src/lang/*.json` | 3 textes nouveaux dans les 27 langues : `video_frames_sending`, `video_frames_failed`, `video_frames_abandoned`. |
| Version | 0.3.6 dans `tauri.conf.json`, `Cargo.toml`, `Cargo.lock`, `package.json` et `package-lock.json`. |

---

## Recette pour Fabien — agent DEV 0.3.6, sur dev

1. **Installation.** Installer le `.msi` DEV : le badge affiche « v0.3.6 DEV » et la connexion est conservée.
2. **Brouillon.**
   1. Créer une épreuve sans la publier et sans ouvrir sa page photos. Dans l'agent, elle porte le badge « Brouillon ».
   2. L'ouvrir et filmer 2 à 3 min. L'avis jaune est en haut. Le journal écrit une fois « Clips en attente : l'épreuve est en brouillon… ». La tuile affiche « N images d'analyse — 0 envoyées ».
   3. Publier l'épreuve sur le site. En 10 s au plus :
      - l'avis jaune disparaît ;
      - le journal écrit « Épreuve publiée… » ;
      - la tuile passe à « … 0 envoyées, 10 en cours d'envoi », puis « 10 envoyées », et ainsi de suite jusqu'à « N envoyées ».
   4. Les clips partent comme en 0.3.5.
3. **Épreuve déjà publiée.** Filmer 2 min. Aucun avis jaune. La tuile montre « en cours d'envoi », puis le nombre d'images envoyées augmente.
4. **Non-régression, rapide.**
   - Message d'arrêt « par cette captation ».
   - « Déjà en ligne » compté à part.
   - Export du journal dans `Documents\Attimo Agent Terrain\Journaux`.
   - Bloc HD « 5 / 5 ».
