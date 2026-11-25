# Documentation {#documentation}

| Document | Contenu |
|---|---|
| [BUILDING.md](../BUILDING.md) | Compiler depuis les sources sous Windows, macOS et Linux (et la version ASIO facultative), lancer les projets d'exemple, rendre avec `atomik-render`, les invariants stricts, la carte des crates et les vérifications que lance la CI. |
| [ARCHITECTURE.md](../ARCHITECTURE.md) | Les crates et leur règle de dépendance, le flux de données (du patch au WAV, ouverture d'un projet et synchronisation en direct, import, lecture, streaming, enregistrement, sauvegarde et rendu), le moteur, le modèle de threads, l'hébergement des plugins, le clavier, l'explorateur de fichiers, et la façon dont chaque invariant strict (sécurité temps réel, déterminisme, validité du graphe, originalité) est garanti. |
| [DEVICES.md](../DEVICES.md) | Référence de chaque appareil intégré : prises, paramètres, plages, comportement et limites. |
| [EXTENDING.md](../EXTENDING.md) | Écrire un nouvel appareil avec `atomik-device-sdk` : le contrat, les paramètres, les notes, les tests avec le banc d'essai, et une liste de contrôle. |
| [TESTING.md](../TESTING.md) | Lancer les contrôles de qualité et les tests, la garde d'allocation, les tests de déterminisme, les fichiers de référence et leur régénération, les tests de plugins, et les tests matériels et manuels. |
| [MCP.md](../MCP.md) | Le serveur `atomik-mcp` pour l'assurance qualité : comment le compiler et s'y connecter, ses outils (rendu, analyse et comparaison audio, une session d'application sans fenêtre avec commandes, saisie, textes et captures d'écran), les recettes de test qu'une modification doit suivre, et comment ajouter un outil. |
| [TAB.md](../TAB.md) | Le guide de la tablature de guitare : les 24 instruments et leurs accordages, l'import de Guitar Pro, MusicXML, de tablatures ASCII et de grilles d'accords, l'export en MusicXML, texte et PDF, l'éditeur de tablature, le panneau de grille d'accords, l'entraînement (vitesse, décompte, boucle, entraîneur de vitesse, écoute et notation), le manche, l'entraîneur d'accords, le son Plucked Strings, les exercices, le dépannage et un tableau des commandes. |
| [DEPENDENCIES.md](../DEPENDENCIES.md) | Les versions volontairement figées et pourquoi, les versions en double ignorées, les avis de sécurité ignorés avec leurs dates de revue, et la mise à jour trimestrielle. |
| [DECISIONS.md](../DECISIONS.md) | Le journal des décisions : les questions de conception qui touchent plusieurs chantiers (traitement par lots des plugins, licence ASIO, compensation de latence, format des patchs, modulation, nom des notes, cadence des formats et autres), avec les options, les conséquences et l'état de chacune. |
| [KEYBOARD.md](KEYBOARD.md) | La référence du clavier. Générée par `atomik --print-keymap --lang fr` ; un test la garde identique à la sortie du programme (la régénérer avec `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-app --lib cli::`). |

## La fenêtre principale {#the-main-window}

La fenêtre principale réunit la barre de transport, l'arrangement, le rack, la table de mixage et l'explorateur de
fichiers. Chaque partie explique ses commandes dans le mode aide, qui renvoie ici.

- **La barre de transport.** Lecture, Arrêt, Enregistrer, Boucle et le métronome, le tempo et la mesure, les repères
  de la plage de boucle, le tempo au tap et Q Rec (il cale sur une grille les notes que vous enregistrez). La barre
  garde ce qui tient dans sa largeur, le reste se trouve dans le menu à son extrémité droite.
- **L'arrangement.** Une ligne par piste, avec les boutons Mute, Solo et Armer, les voies d'automation dessous et les
  clips dans le temps. Les outils du coin (Sélection, Crayon, Gomme, Rasoir, Enceinte, Mute, Loupe et Main) changent
  ce que font un clic et un glissé, et la grille d'aimantation à côté d'eux règle l'endroit où se posent les clips et
  les points.
- **Le piano roll.** S'ouvre sur un clip de notes. Il a les outils de sélection, crayon, gomme et rasoir, une grille
  d'aimantation, la quantification, une gamme avec une tonique et un type sur lesquels les notes peuvent s'aimanter,
  des tampons d'accord, et les voies de contrôleurs sous la voie de vélocité.
- **Le bandeau d'expression.** Une note peut porter ses propres courbes de hauteur, de pression et de timbre (les
  trois dimensions du MPE, enregistrées depuis un clavier MPE ou dessinées ici). Sélectionnez une note qui a des
  courbes, ou lancez Éditer l'expression des notes depuis la palette de commandes pour n'importe quelle note (cela
  marche aussi avec plusieurs notes sélectionnées), et un bandeau s'ouvre à côté, avec un onglet pour chaque courbe.
  Cliquez dans l'espace vide pour ajouter un point et faites glisser un point pour le déplacer (Maj le déplace
  finement, Ctrl cale une hauteur sur le demi-ton) ; Alt-clic ou double-clic sur un point le supprime ; faites
  glisser depuis l'espace vide pour sélectionner plusieurs points et les déplacer ou les supprimer ensemble ; avec
  l'outil Crayon, un glissé dessine la courbe à main levée. Le bouton d'échelle de hauteur passe successivement par
  48, 24, 12 et 2 demi-tons de chaque côté. Appliquer copie la courbe de la première note sélectionnée vers les
  autres notes sélectionnées, étirée à la longueur de chacune. Chaque geste est une étape d'annulation. Les courbes
  sortent en MPE : réglez l'appareil de sortie MIDI sur une zone basse ou haute, ou appuyez sur MPE dans le panneau
  d'un plugin qui ne prend que du MIDI ordinaire ; les plugins qui acceptent les expressions de notes reçoivent les
  courbes sans cela.
- **La table de mixage.** Une tranche par canal, avec ses sections (entrée, égaliseur, compresseur, envois, pan et
  fader), les emplacements d'insert, le choix de la sortie, des instantanés de tout le mixage et des groupes qui
  déplacent plusieurs faders ensemble.
- **L'explorateur de fichiers.** Les emplacements à gauche, les fichiers à droite. Il trie par nom, durée ou taille,
  écoute un aperçu de l'audio quand vous sélectionnez un fichier, et avance et recule comme un navigateur web.
- **Le rack.** Chaque appareil a un nom, un bouton de repli et ses propres commandes. Survolez un bouton en mode aide
  pour lire sa plage et ce que fait son appareil.

## Le mode aide {#help-mode}

Aide > Mode aide, ou Maj+F1, l'active et le désactive (F1 seul ouvre la liste des raccourcis clavier). Tant qu'il est
actif, un panneau d'aide se place au-dessus de la ligne d'état et le morceau lui cède la place. Pointez une commande
et le panneau affiche :

- **Le titre et le texte.** Le nom de la commande et une à quatre phrases à son sujet. Un bouton, un interrupteur ou un
  choix d'appareil affiche son nom, sa plage et son unité, et une ligne sur l'appareil auquel il appartient.
- **Le raccourci.** La touche de la commande que lance la commande pointée, lue dans votre clavier, de sorte qu'une
  touche réassignée s'affiche.
- **Plus.** Un lien vers la section de cette documentation qui en parle ; cliquez dessus pour ouvrir la section dans
  le manuel intégré (voir [Manuel](#manual)). Rien n'est envoyé nulle part.

Rien n'est bloqué : les clics, les glissés et les touches fonctionnent comme d'habitude, et les info-bulles ordinaires
restent désactivées tant que le mode est actif. Le panneau garde la dernière commande qu'il a expliquée tant que le
pointeur est sur un espace vide ou sur le panneau lui-même, afin que le lien reste accessible. Une commande qui n'a
pas encore de texte d'aide affiche à la place le texte de son info-bulle. Le mode n'est pas enregistré ; chaque
session démarre avec lui désactivé.

## Le manuel {#manual}

Aide > Manuel (Ctrl+F1, ou Cmd+Alt+F1 sur macOS) ouvre cette documentation dans l'application. Il fonctionne hors
connexion : le texte est le même fichier que celui de la documentation de la page du projet, compilé dans le
programme, si bien que les deux ne diffèrent jamais. La palette de commandes liste chaque section sous la forme
« Manuel : <titre> », et le lien Plus du mode aide ouvre le manuel à sa section. Quand l'interface est en français, le
manuel s'ouvre en français ; le sélecteur « English / Français » en haut du manuel permet de lire l'autre langue.

- **La table des matières.** À gauche, construite à partir des titres des documents : celui-ci, puis la référence du
  clavier. Cliquez sur une ligne pour y aller ; la molette fait défiler la liste.
- **La page.** À droite : titres, paragraphes, texte en gras et en italique, code, listes, tableaux et liens entre
  sections, dessinés aux couleurs du thème. Une image est montrée sous la forme de sa description. Haut et Bas, Page
  préc. et Page suiv., Début et Fin, la molette et la barre de défilement permettent de parcourir la page.
- **La recherche.** Tapez dans le champ du haut : chaque mot doit figurer dans un titre, ou dans un paragraphe, un
  élément de liste, un tableau ou un bloc de code d'une section. Les résultats sont classés : un titre égal à ce que
  vous avez tapé vient en premier, puis les titres qui commencent par ces mots, puis les autres titres, puis les
  sections où les mots reviennent le plus. Chaque résultat montre une ligne autour de la correspondance, avec les
  mots trouvés en gras. Haut et Bas choisissent un résultat, Entrée ou un clic l'ouvre, et Échap efface la recherche
  (un second Échap ferme le manuel). La recherche porte sur la langue affichée.
- **Précédent et Suivant.** Les deux boutons à gauche du champ de recherche, ou Alt+Gauche et Alt+Droite, parcourent
  les endroits où vous êtes passé, comme dans un navigateur web. Un lien, une ligne de la table des matières et un
  résultat ajoutent chacun un endroit.
- **Les liens.** Un lien vers une section du manuel y mène. Un lien vers une page que le manuel ne contient pas (la
  référence des appareils, les autres documents du projet) ouvre cette page dans le navigateur, par la même requête
  unique vers une adresse fixe que celle d'Aide > Documentation. Aide > Documentation ouvre toujours la page du
  projet. Ces documents ne sont écrits qu'en anglais.

Échap, un clic en dehors du panneau, ou de nouveau Ctrl+F1 ferme le manuel.

## Les appareils {#devices}

Chaque appareil intégré a une face avant dans le rack, et le navigateur d'appareils les liste par genre : instruments,
effets, players et mixeurs. Le mode aide explique les boutons, interrupteurs et choix d'un appareil : pointez-en un
pour lire sa plage, son unité et une ligne sur l'appareil auquel il appartient. Les plugins aux formats VST3 et CLAP
se placent dans le rack de la même façon (voir [Plugins](#plugins)).

La référence de chaque appareil intégré est [DEVICES.md](../DEVICES.md) : ses prises, ses paramètres, ses plages, son
comportement et ses limites. Elle est longue, le manuel renvoie donc vers elle sur la page du projet au lieu d'en
contenir une copie.

## Le lanceur {#launcher}

Le lanceur est une seconde façon de faire de la musique à côté de l'arrangement : une grille de clips en boucle que
vous démarrez et arrêtez en direct, et qui peuvent être écrits dans l'arrangement. Affichage > Lanceur (F10) l'affiche
à la place du rack, l'arrangement restant en dessous ; la même commande le masque à nouveau. Tant que le rack n'est pas
dans la fenêtre principale, le lanceur prend à sa place le panneau du bas.

- **La grille.** Chaque piste de notes ou audio est une colonne, dans l'ordre et les couleurs de la liste des pistes de
  l'arrangement ; chaque ligne est une scène. Un emplacement contient un clip : son nom, une petite image de ses notes
  ou de sa forme d'onde et un triangle de lecture. La colonne des scènes, à droite, a un bouton de lancement et le nom
  de la scène (double-clic pour la renommer) ; la rangée sous la grille a un carré d'arrêt pour chaque piste et TOUT
  ARRÊTER. Un emplacement qui joue a une barre le long de son bord inférieur qui suit la boucle, et un emplacement qui
  attend son moment est cerclé et clignote. Les emplacements appartiennent aux pistes de notes et aux pistes audio.
- **Faire des clips.** Double-cliquez sur un emplacement vide pour créer un clip de notes d'une mesure et l'ouvrir dans
  le piano roll ; double-cliquez sur un clip pour l'ouvrir. Faites glisser un emplacement vers un autre pour le
  déplacer, avec Ctrl (Cmd sur macOS) enfoncé pour le copier. Le menu du clic droit d'un emplacement le renomme, le
  colore, règle la longueur de sa boucle (1 à 32 mesures), règle sa quantification du lancement et le duplique ou le
  supprime ; celui d'une scène insère, duplique et supprime des scènes. Édition > Copier, Couper, Coller, Dupliquer et
  Supprimer agissent sur l'emplacement sous le curseur. Affichage > Lanceur > Clip vers le lanceur place le clip
  d'arrangement sélectionné dans l'emplacement de sa piste à la ligne du curseur, et Clip du lanceur vers
  l'arrangement place le clip sous le curseur à la tête de lecture.
- **Lancer.** Cliquez sur le triangle d'un emplacement, ou déplacez le curseur avec les flèches et appuyez sur
  Entrée ; Maj+Entrée, ou le triangle de la colonne des scènes, lance toute la scène. Un lancement attend la
  prochaine limite de la quantification du lancement (le bouton à droite de la barre du haut la fait défiler ;
  Affichage > Lanceur > Quantification du lancement la liste ; Aucune, 1/16, 1/8, 1/4, 1/2, 1 mesure, 2 mesures ou
  4 mesures, 1 mesure par défaut ; un emplacement peut avoir la sienne) et les limites sont exactes à travers les
  changements de tempo et de mesure. Le clip joue ensuite depuis son propre début et boucle. Si le transport est à
  l'arrêt, le lancement le démarre là où il se trouve ; une position sur une barre de mesure lance aussitôt.
- **Lancer par-dessus l'arrangement.** Tant qu'une piste joue un emplacement, ses clips d'arrangement sont muets. La
  piste rejoue son arrangement quand son emplacement s'arrête (le carré d'arrêt sous la piste, ou Arrêter tous les
  clips : les deux attendent la quantification du lancement) ou aussitôt avec RETOUR À L'ARRANGEMENT, qui est allumé
  tant qu'une piste joue un emplacement. Avec La scène arrête les pistes vides activé (par défaut), le lancement d'une
  scène arrête aussi les pistes qui jouent un emplacement et n'en ont aucun dans cette ligne. Les notes qui sont
  actives quand un emplacement s'arrête ou qu'un autre démarre sont relâchées.
- **Enregistrer les lancements.** Activez ENREGISTRER LES LANCEMENTS (ou Affichage > Lanceur > Enregistrer les
  lancements), puis appuyez sur Enregistrer. Tout ce que vous lancez à partir de là est écrit dans l'arrangement à
  l'arrêt : chaque lancement devient un clip des notes de l'emplacement à la position du morceau où il a réellement
  commencé, aussi longtemps qu'il a joué, en boucle quand il a duré plus que l'emplacement. L'ensemble de la passe est
  une seule étape d'annulation. Aucune piste n'a besoin d'être armée ; avec des pistes armées, les notes que vous
  jouez sont enregistrées comme d'habitude à côté. Appuyez ensuite sur RETOUR À L'ARRANGEMENT pour écouter
  l'enregistrement.
- **Audio et glissés.** Un emplacement audio contient un clip audio (son fichier, son gain et ses fondus) et le joue
  sans warp, en boucle sur la longueur de l'emplacement. Déposez un fichier audio sur un emplacement audio, ou un
  fichier MIDI sur un emplacement de notes, pour en faire son clip ; faites glisser un clip d'arrangement sur un
  emplacement pour l'y copier, et un emplacement sur une voie d'arrangement pour y écrire un clip.
- **Pas encore fait.** L'audio warpé dans les emplacements, le lancement depuis un contrôleur MIDI, les actions de
  suite, et les tempos de scène qui changent le tempo du morceau. Les notes d'un emplacement lancé vont à l'instrument
  de la piste comme n'importe quelles autres notes, si bien qu'un clavier MIDI le joue toujours.

## Tonalité et accords {#key-and-chords}

Le morceau peut avoir une tonalité et une voie d'accords. Le reste de l'application les suit : le piano roll,
l'explorateur, les nouveaux accords et quelques outils. Ils ne changent le son du morceau qu'à travers les appareils
que vous demandez de les suivre (plus bas).

- **La tonalité du morceau.** Une tonique et une gamme (majeure, mineure, les modes, les gammes pentatoniques et les
  autres du piano roll ; jamais chromatique). Cliquez sur le champ Tonalité de la barre de transport, ou choisissez
  Transport > Tonalité du morceau..., pour ouvrir le sélecteur et la régler. Le champ montre la tonalité à la tête de
  lecture, il montre donc aussi un changement de tonalité une fois qu'on en a passé un. Sur une barre étroite, le
  champ disparaît et la liste « ... » a une ligne Tonalité du morceau....
- **Les changements de tonalité.** La voie de tonalité (Affichage > Voies et marqueurs > Voie de tonalité) se place
  au-dessus des pistes, sous les voies de tempo et d'énergie. Un clic sur la voie vide ajoute un changement de
  tonalité au temps aimanté, avec la tonalité déjà en vigueur à cet endroit ; un double-clic ouvre le sélecteur dessus
  (ou sur un nouveau) pour en choisir une autre. Faites glisser une étiquette pour la déplacer. L'étiquette du début
  est la tonalité du morceau. La tonalité à un instant donné est le dernier changement situé avant lui ou à cet
  instant, sinon la tonalité du morceau.
- **La voie d'accords.** Affichage > Voies et marqueurs > Voie d'accords. Un clic sur la voie vide y ajoute l'accord
  de tonique de la tonalité (mineur dans une tonalité mineure), long d'une mesure ou moins quand l'accord suivant est
  proche ; gardez le bouton enfoncé et faites glisser pour régler la longueur. Faites glisser un accord pour le
  déplacer, faites glisser son extrémité droite pour le redimensionner, double-cliquez dessus pour le changer. Les
  accords ne se chevauchent jamais : un déplacement ou un redimensionnement qui buterait sur l'accord suivant s'arrête
  avant. Échap annule un glissé, et chaque geste est une étape d'annulation. Le bouton droit sur un accord ou une
  étiquette liste Supprimer et Copier ; sur la voie vide, il liste Ajouter ici et, après une copie, Coller. Coller
  l'accord (Ctrl+Alt+V dans l'arrangement, ou la palette de commandes) place l'accord copié à la tête de lecture.
- **Entendre un accord.** Un clic sur un accord le joue un instant avec l'instrument de la piste de notes sélectionnée,
  ou une petite voix intégrée quand aucune n'est sélectionnée ; choisir un accord dans le sélecteur le joue aussi. Il
  reste muet pendant la lecture du morceau. Désactivez-le avec Jouer les accords au clic dans le menu du clic droit de
  la voie d'accords.
- **Le sélecteur.** La rangée Tonique, les rangées Qualité (ou les rangées Gamme pour une tonalité) et la rangée Basse
  choisissent un accord à la souris. Le champ Accord accepte un symbole : une lettre de tonique (A à G, en majuscule ou
  en minuscule) avec un dièse ou un bémol, une qualité comme m, min, -, maj7, M7, 7, m7b5, dim, dim7, aug, sus2, sus4,
  6, m6, 9, maj9, m9, add9, 11, 13 ou 5, et une note de basse facultative après une barre oblique : Am7, F#m7b5,
  Bbmaj7, Dsus4/A, C/G. Les boutons et le champ se suivent l'un l'autre, et un symbole qu'il ne sait pas lire est
  expliqué sous le champ tant qu'Appliquer attend. Retirer supprime l'accord, le changement de tonalité ou la tonalité
  du morceau.
- **Le piano roll suit la tonalité.** Un clip qui n'a pas de gamme propre montre la gamme de la tonalité du morceau au
  début du clip : la mise en évidence de la gamme, l'aimantation à la gamme et le repli fonctionnent avec elle, et la
  barre de gamme l'affiche. Une gamme que vous réglez sur le clip l'emporte toujours. Un nouveau clip de notes n'a pas
  de gamme, il suit donc le morceau jusqu'à ce que vous lui en donniez une.
- **Suivre la tonalité du morceau.** Le menu Gamme du piano roll a une case Suivre la tonalité du morceau, cochée pour
  chaque clip. Décochez-la et le clip n'est plus ni mis en évidence ni aimanté par la tonalité du morceau, sans gamme
  propre pour la remplacer (une gamme que vous réglez compte toujours). C'est une étape d'annulation par clic, et cela
  ne change que l'éditeur : les appareils qui suivent la tonalité du morceau (Tonalité et accords, le Quantificateur
  CV) continuent de la suivre pour les notes de ce clip, car une note qui arrive à un appareil ne dit pas de quel clip
  elle vient.
- **Les accords dans le piano roll.** Le menu Tampon d'accord a Accord du morceau : le crayon dépose l'accord de la
  voie d'accords à l'endroit où vous dessinez (une seule note là où la voie n'a pas d'accord), avec sa note de basse
  sous l'accord. Ajuster les notes aux accords (dans le menu Accords) déplace chaque note sélectionnée qui sort de
  l'accord à sa position vers la note d'accord la plus proche, un ex æquo allant vers le bas, et jamais sur une note
  déjà présente. Les notes déplacées apparaissent d'abord en contours, avec Appliquer et Annuler dans une barre en haut
  du piano roll (Entrée applique, Échap annule) ; Appliquer est une étape d'annulation.
- **Les appareils qui suivent le morceau.** Tonalité et accords peut prendre sa gamme dans la tonalité du morceau
  (Source de la gamme : morceau, changements de tonalité compris) et son accord dans la voie d'accords (Source de
  l'accord : accord du morceau) : Suivre le tampon joue l'accord de la voie autour de toute touche que vous
  pressez, Suivre l'aimantation déplace chaque touche vers la note d'accord la plus proche, et Redéclencher refait la
  voix des touches que vous tenez quand l'accord change. La Source de l'Arpégiateur joue l'accord de la voie (accord
  du morceau) ou aimante les touches tenues dessus (tenues + accord du morceau) ; avec Selon les touches désactivé, il
  tourne tout seul chaque fois que le morceau joue un accord, à partir de l'Octave de l'accord. Le Quantificateur CV
  peut quantifier sur la tonalité du morceau. Un appareil qui suit affiche une étiquette MORCEAU avec l'accord ou la
  tonalité à la tête de lecture. Les rendus sonnent exactement comme la lecture.
- **Les accords d'un fichier MIDI.** Importer un fichier MIDI comme morceau lit aussi les symboles d'accords qui y sont
  écrits (événements de texte, de paroles ou de marqueur comme Am7 ou C/G, chacun durant jusqu'au suivant) vers la
  voie d'accords, et son armure comme tonalité du morceau quand le morceau n'en a pas, le tout dans l'unique étape
  d'annulation de l'import. Fichier > Importer > Détecter les accords d'après les notes (désactivé par défaut, gardé
  jusqu'à ce que vous quittiez) ajoute une estimation pour un fichier qui ne nomme aucun accord : à chaque temps,
  chaque classe de hauteur compte selon la durée et la force de ses notes, comparée aux mêmes formes d'accords que
  Détecter les accords dans l'audio, la note la plus grave servant de basse. Les symboles d'accords écrits dans le
  fichier l'emportent toujours sur l'estimation.
- **Détecter la tonalité du morceau.** Transport > Détecter la tonalité du morceau lit chaque clip de notes et règle
  la tonalité qu'il trouve, en une étape d'annulation : plus une note est longue, plus elle compte. La ligne d'état dit
  à quel point la réponse était nette ; Annuler remet l'ancienne tonalité. C'est une estimation, et elle ne trouve que
  le majeur et le mineur naturel. Un morceau sans notes mais avec des clips audio est lu à partir de son audio, et
  Transport > Détecter la tonalité dans l'audio écoute toujours l'audio : les clips audio sélectionnés, ou tous les
  clips audio, dix minutes au plus, sur un thread de travail.
- **Détecter les accords dans l'audio.** Sélectionnez des clips audio (ou réglez la boucle sur la partie voulue) et
  choisissez Transport > Détecter les accords dans l'audio..., ou le même élément du menu du clic droit d'un clip
  audio. Le programme écoute sur un thread de travail (ANNULER l'arrête) et ouvre un panneau avec les accords entendus
  dessinés au-dessus de leurs mesures ; un accord dont il n'est pas sûr est estompé. CHANGEMENTS laisse un accord
  changer à chaque temps, à chaque demi-mesure ou à chaque mesure ; ACCORDS choisit Simple (triades seulement) ou
  Complet (septièmes et sixtes aussi, quand elles sont nettement là) ; ACCORDS AVEC BASSE ajoute la note de basse
  quand ce n'est pas la fondamentale (C/G). Chaque changement s'affiche aussitôt, sans réécouter. REMPLACER LA PLAGE
  place les accords sur l'étendue que couvrent les clips, en remplaçant les accords qui s'y trouvent ; COMBLER LES
  VIDES garde les accords déjà dans la voie et remplit les trous. Quand le morceau n'a pas de tonalité, une case règle
  aussi la tonalité vers laquelle l'audio tend. C'est une étape d'annulation dans les deux cas. Cela marche mieux sur
  une partie claire de piano, de guitare ou de pad ; la batterie et un mixage chargé le font deviner davantage, et un
  enregistrement un peu haut ou un peu bas (A = 432, par exemple) est entendu juste.
- **L'explorateur.** Tapez key:song dans la recherche de l'explorateur de fichiers pour voir les boucles marquées de la
  tonalité du morceau (les boucles d'usine portent un tag comme A minor). Un morceau sans tonalité ne filtre rien.
  key:minor trouve les tags qui contiennent ce mot.
- **Enregistrement.** La tonalité, les changements de tonalité et les accords ne sont enregistrés que s'ils sont
  réglés, sous forme de champs facultatifs qu'une version plus ancienne du programme ignore avec un avertissement
  (format de projet 45).

## Audio vers notes {#audio-to-notes}

Audio > Audio vers notes... transforme les clips audio sélectionnés en clips de notes, sur une nouvelle piste ou sur la
piste de notes sélectionnée. MODE choisit ce qu'il faut écouter : Avec hauteur (une voix), Batterie ou Polyphonique
(accords), qui suit une partie de piano ou de guitare à plusieurs notes à la fois. VOIX SIMULTANÉES plafonne le nombre
de notes qui sonnent ensemble (1 à 6, 4 par défaut), et la sensibilité, la note la plus courte et la plage de notes
fonctionnent comme pour une seule voix. Les notes polyphoniques sont une ébauche à retoucher, pas une transcription :
un mixage dense, une guitare saturée ou deux notes à l'octave peuvent perdre une note ou en ajouter une.

## Séparation des stems {#stem-separation}

Faites un clic droit sur un clip audio et choisissez **Séparer les stems** (aussi dans la palette de commandes,
`audio.separate_stems`, sans touche par défaut) pour le scinder en voix, batterie, basse et tout le reste.
Sélectionnez d'abord un clip audio. Une boîte de dialogue demande quels stems créer (les quatre sont cochés) et s'il
faut **Couper le son du clip d'origine** (activé). Chaque stem devient une nouvelle piste audio juste sous la piste du
clip, nommée d'après le clip (« Verse Vocals », « Verse Drums »...), avec une copie du clip au même endroit qui a le
même décalage, le même gain, les mêmes fondus et le même warp mais joue le fichier du stem ; les quatre stems
s'additionnent pour redonner le clip là où les modèles entendent quelque chose. L'ensemble est une étape d'annulation,
et les fichiers de stems vont dans le dossier audio du projet comme n'importe quel audio rendu.

- **Comment cela marche, et pourquoi il n'y a pas de modèle.** C'est une séparation rapide par traitement du signal,
  pas un modèle appris : il n'y a donc rien à télécharger et rien ne quitte votre ordinateur. La batterie est la partie
  percussive d'une séparation harmonique/percussive, la basse est la partie harmonique sous 150 Hz environ, la voix est
  la partie harmonique au-dessus qui se trouve au centre de l'image stéréo, et le reste est ce qui reste, si bien que
  les quatre stems s'additionnent pour donner le clip. Un modèle dont les poids ou les données d'entraînement portent
  des conditions non commerciales n'est pas utilisé, et aucun n'est connu dont les poids et les données en soient
  exempts ; la boîte de dialogue dit « Signal-processing split: expect some bleed between stems. ».
- **Vitesse et mémoire.** Elle tourne sur un thread de travail avec une progression et Annuler pendant que le morceau
  continue de jouer. Un morceau stéréo de 3 minutes prend quelques secondes dans une version release ; le travail se
  fait par morceaux de 20 secondes, un long fichier n'a donc pas besoin de plus de mémoire pour la séparation. Le même
  clip donne toujours les mêmes stems.
- **Limites.** Attendez-vous à des fuites : une grosse caisse fuit dans la basse, une queue de caisse claire ou un
  charleston dans les autres stems, une guitare ou un synthé au centre dans la voix, et les queues de réverbération
  sont partagées entre les stems. Un fichier mono n'a pas d'image stéréo pour distinguer la voix du reste, sa voix n'est
  donc que la bande de 200 Hz à 5 kHz, à un poids plus faible (la boîte de dialogue le dit). Tout le fichier du clip
  est séparé, pas seulement la partie que joue le clip. Un fichier mono donne des stems mono ; avec plus de deux canaux,
  les deux premiers sont utilisés. Un clip sur une piste gelée est refusé, ainsi que plusieurs clips à la fois.

## Mode voix {#vocal-mode}

L'éditeur de clip audio (double-clic sur un clip audio, ou F8) a un mode Voix pour un clip à une seule voix à la fois :
un chanteur, une basse, un lead. Appuyez sur VOIX dans sa barre d'outils, ou sur 3.

- **Les notes.** L'éditeur écoute le clip et trouve ses notes ; le pied de l'éditeur montre l'avancement et ANNULER (ou
  Échap) l'arrête. Ce n'est pas enregistré : cela est refait à l'ouverture du clip, et rapidement pour un clip qu'il
  vient de voir. SENS - et + trouvent moins ou plus de notes. Un clip sans hauteur nette (batterie, bruit, accord)
  affiche « No pitched notes found in this clip ».
- **La grille.** La forme d'onde devient une grille de demi-tons avec des touches de piano à gauche, estompée derrière
  les notes. Quand le morceau a une tonalité, sa gamme est allumée. Chaque note est une barre de son début à sa fin à sa
  hauteur, avec la courbe de hauteur qui la traverse : l'originale pâle, le résultat vif.
- **Sélectionner.** Cliquez sur une note ; Maj-clic en ajoute ou en retire une ; faites glisser sur la grille vide pour
  tracer un cadre. Ctrl+A sélectionne tout.
- **Déplacer la hauteur.** Faites glisser une note vers le haut ou le bas : les notes sélectionnées se déplacent par
  demi-tons entiers. Maintenez la touche d'outil alternatif (Alt, Ctrl sur macOS) pour les déplacer librement, en
  cents. Échap pendant le glissé les remet en place.
- **Déplacer dans le temps.** Faites glisser une note de côté : deux marqueurs de warp sont épinglés à son début et à sa
  fin et déplacés ensemble, si bien que la note bouge et que l'audio de part et d'autre s'étire. Un clip sans warp
  l'a activé au tempo du morceau.
- **CORRIGER** amène le centre de chaque note sélectionnée au demi-ton le plus proche ; avec **GAMME** activé et une
  tonalité du morceau, à la note de la gamme la plus proche. **APLATIR** redresse la dérive de hauteur des notes
  sélectionnées de la quantité indiquée à côté (faites glisser le nombre vers le haut ou le bas, 100 % donne une ligne
  droite) ; un vibrato est conservé. **RÀZ** supprime les retouches des notes sélectionnées, ou de toutes les
  notes quand aucune n'est sélectionnée. Chacun est une étape d'annulation, de même que chaque glissé.
- **Le pied de l'éditeur** nomme la première note sélectionnée (E3, par exemple) avec son écart au demi-ton le plus
  proche en cents, avant et après la retouche, et sa dérive.
- **Le son.** Les retouches sont épinglées à l'audio, elles restent donc avec leurs notes quand le tempo ou les
  marqueurs de warp changent, une coupe garde chacune dans le morceau qui contient le début de la note, et une
  jonction garde les deux. Le clip est rendu avec elles en arrière-plan (le clip affiche CALC en attendant et joue sans
  les retouches jusqu'à la fin) ; le rendu, Rendre sur place, Geler et Exporter les entendent toujours. La
  transposition garde le caractère de la voix (ses formants) : elle travaille sur une note à la fois, elle convient donc
  à une voix et non à des accords.
- **Enregistrement.** Les retouches sont enregistrées avec le clip, sous forme de champ facultatif qu'une version plus
  ancienne ignore avec un avertissement (format de projet 45).

## Mode comp {#comp-mode}

Un clip qui a plusieurs prises (enregistrement en boucle, ou enregistrement par-dessus un clip) les garde dans un
groupe de prises, et l'arrangement affiche le bandeau de comp sous la voie. L'éditeur de clip audio a le même comp dans
une vue plus grande et exacte : ouvrez une prise ou le clip compilé et appuyez sur COMP dans sa barre d'outils. La voie
de comp, en haut, montre quelle prise joue où (un trou est un silence), et dessous chaque prise a une voie avec sa forme
d'onde, la plus récente en haut ; les parties d'une prise que le comp joue sont teintées.

- **Écouter une prise.** Appuyez sur la voie d'une prise pour l'entendre depuis le pointeur jusqu'au relâchement, telle
  qu'elle est dans son fichier. Rien ne change.
- **Choisir.** Faites glisser sur la voie d'une prise pour mettre la plage glissée de cette prise dans le comp. La
  coupure reçoit un fondu enchaîné, de 10 ms au départ ; FONDU ENCH - et + changent sa longueur pour les suivantes (le
  bandeau de comp de l'arrangement règle le fondu enchaîné d'une coupure existante). La plage reste sélectionnée.
- **Touches.** Haut et Bas déplacent la plage sélectionnée vers la prise du dessus ou du dessous ; sans plage, ils
  déplacent la voie sélectionnée. Cliquez sur la voie de comp pour sélectionner le tronçon qui y joue. Échap pendant un
  glissé ne choisit rien.
- **Aplatir le comp.** APLATIR LE COMP rend le comp dans un nouveau fichier audio du dossier audio du projet et
  remplace le groupe de prises par un clip de ce fichier, en une étape d'annulation. Le rendu se fait en arrière-plan
  (le même que celui dont se sert Joindre les clips de fichiers différents) avec le gain, les fondus et l'enveloppe de
  chaque prise et les fondus enchaînés aux coupures, avant les effets d'insert de la piste. Les prises partent avec le
  groupe : Annuler les ramène.
- Chaque choix, et Aplatir le comp, est une étape d'annulation. Le mode comp demande des prises audio : un groupe de
  prises de notes le dit.

## Garder les formants {#keep-formants}

Le mode Vue d'ensemble de l'éditeur de clip audio a trois commandes pour un clip warpé en mode Tonal (réglez le mode
dans le panneau de warp). Elles sont enregistrées avec le clip, et le rendu, Rendre sur place, Geler et Exporter les
entendent.

- **GARDER LES FORMANTS** garde l'enveloppe spectrale du son (ses formants, le caractère d'une voix ou d'un
  instrument) là où elle était pendant que le clip est transposé, de sorte qu'un accord transposé ou un clip entier ne
  se change pas en voix de souris ou de géant. Cela marche sur n'importe quelle matière, là où la transposition du mode
  Voix demande une seule voix à la fois.
- **DÉCALAGE** déplace les formants seuls, jusqu'à 12 demi-tons de chaque côté, avec GARDER LES FORMANTS activé : une
  voix plus grave ou plus brillante à la même hauteur.
- **LIEN STÉRÉO** garde la phase entre la gauche et la droite pendant que le clip est étiré, pour que l'image stéréo
  reste. Il est désactivé pour les clips faits avant qu'il existe, afin que leur son ne change pas.

L'enveloppe est déplacée dans le tampon rendu. Tant que le tampon est encore en cours de rendu, l'aperçu en direct joue
le clip avec les formants qui suivent la hauteur, comme avant.

## Panneaux de mesure {#meter-panels}

Cliquez sur le petit bouton **FFT** dans le coin supérieur droit de la plaque de nom d'un canal, d'un bus ou du master
dans la console pour ouvrir son panneau de spectre (cliquez dessus à nouveau pour le fermer ; le bouton est allumé tant
qu'un panneau de cette tranche est ouvert). Faites un clic droit sur le bouton pour choisir le spectre ou le
phasemètre. Le menu d'une tranche ou d'un appareil plugin dans le rack a aussi **Afficher le spectre** et **Afficher le
phasemètre**. Un panneau avec le dessin des appareils d'analyse s'ouvre au-dessus de la fenêtre ; rien n'est ajouté au
morceau. Il montre la sortie de la tranche après son fader (pour un appareil plugin : sa sortie après le contournement),
on le déplace en le faisant glisser par son titre, on peut en ouvrir plusieurs, et il se ferme avec la tranche ou
l'appareil auquel il appartient. Faites glisser la poignée de son coin inférieur droit pour l'agrandir, ou appuyez sur
le carré à côté de la croix de fermeture (ou double-cliquez sur le titre) pour remplir la fenêtre ; recommencer le
remet en place. Les panneaux ne sont pas enregistrés.

- **Spectre.** Les flèches de TAILLE font passer la FFT de 1024 à 65536 points : plus grande, elle montre plus de détail
  et bouge plus lentement. Quand le pointeur est sur le tracé, une lecture indique la fréquence, la note la plus proche
  et le niveau à cet endroit. **MAINTIEN** garde les crêtes sous forme d'une ligne fine qui descend lentement ; cliquez
  sur le tracé pour la relancer. Un panneau plus grand dessine une trace plus fine, plus d'étiquettes de fréquence et
  l'échelle en dB. Le bouton **RÉF** ouvre une liste : une autre tranche, ou la piste de référence, dessinée comme
  seconde courbe sur la trace pour pouvoir comparer les deux. La piste de référence est dessinée au niveau où elle est
  entendue (le gain calé et le trim). L'appareil Spectre a le même bouton RÉF sur son tracé, et garde son choix dans
  le projet.
- **PRÉ et POST.** La pastille à côté des flèches de taille prend la tranche avant son fader (PRÉ) ou après (POST, par
  défaut), pour voir ce que fait le fader. Un appareil plugin est pris avant le plugin ou après.
- **Grandes tailles.** Les deux plus grandes tailles demandent plus d'historique que ce qu'une tranche garde : la
  première fois qu'un panneau en choisit une, la tranche reçoit un tampon de 1 Mio pour cela, et une fenêtre qui n'est
  pas encore remplie montre ce qu'elle a.
- **Phasemètre.** Le goniomètre des images les plus récentes, la corrélation sous forme d'une barre et d'un nombre, et
  le niveau de chaque côté.
- **Coût.** Une tranche n'écrit que ce que lit le panneau tant qu'un panneau est ouvert ; sans aucun panneau ouvert, le
  thread audio ne fait rien de plus, et un rendu est identique au bit près.

## Pistes de référence {#reference-tracks}

Table de mixage > Référence contient jusqu'à 16 fichiers de référence par morceau, une touche pour basculer entre le
mixage et la référence sélectionnée, et le panneau Références et Zone de décision. Pendant que vous écoutez une
référence qui suit la tête de lecture, déplacer la tête de lecture déplace la référence à la même position du morceau,
et le saut en arrière à la fin d'une boucle pendant que le morceau joue avec la boucle activée fait de même ; une
référence avec son propre début (Démarrer à la position de la tête de lecture) reste où elle est. La rangée **TRIM** du
panneau ajoute jusqu'à 12 dB de chaque côté au niveau calé, 0,1 dB par clic (1 dB avec Maj), et est enregistrée avec la
référence. **CALER SUR LA SÉLECTION** mesure le volume sonore du mixage et de la référence sur la sélection de temps (ou
la boucle, quand rien n'est sélectionné) et règle le niveau calé d'après eux ; désactivez puis réactivez Caler sur le
niveau de référence pour revenir au calage lent de la section la plus forte.

## Commande par webcam {#webcam-control}

La webcam peut tourner un bouton : cliquez sur un bouton, un curseur ou un fader (sur un panneau du rack, dans le
panneau de paramètres, sur une tranche de la console ou dans la fenêtre propre d'un plugin), pincez le pouce et l'index
devant la caméra et tournez le poignet comme un bouton. La dernière commande sur laquelle vous avez appuyé est la
**cible de la caméra** : elle porte une petite lentille et un anneau tant que la caméra est allumée, c'est un état de
vue (jamais enregistré avec un projet) et supprimer son appareil l'efface.

- **L'allumer.** Le bouton CAM de la barre de transport (quand la barre a la place ; le menu `...` l'a aussi),
  Affichage > Panneaux > Caméra et la commande `camera.toggle` (sans touche par défaut). La caméra est éteinte tant que
  vous ne l'allumez pas, et elle est de nouveau éteinte quand vous l'éteignez, quand vous quittez et à la moindre erreur
  de caméra. La première fois, les modèles de la main manquent, et une boîte de dialogue dit ce qui est téléchargé (deux
  fichiers, 7,8 Mo), d'où (le nom d'hôte), que rien sur vous ni sur votre ordinateur n'est envoyé et que les images de
  la caméra restent en mémoire. Rien n'est récupéré avant que vous appuyiez sur Télécharger ; il montre la progression,
  Annuler l'arrête et un échec dit pourquoi, avec Réessayer. Les fichiers sont vérifiés par une somme de contrôle, et un
  téléchargement échoué ou annulé ne laisse rien derrière lui. Réglages > Caméra > Supprimer les modèles téléchargés les
  supprime.
- **Le geste.** Un pincement ouvre un geste, chaque rotation tourne la cible, et relâcher le ferme : c'est une étape
  d'annulation, l'automation en mode Touch écrit tant que les doigts sont réunis, et cela fonctionne exactement comme un
  contrôleur MIDI assigné à la commande. Une rotation tourne la cible de son angle sur **Rotation pour toute la plage**
  (30 à 360 degrés, 150 par défaut) de sa plage ; avec **Réglage fin selon la distance** (activé par défaut) une main
  proche de la caméra la tourne moitié moins loin et une main éloignée sur toute la plage. Une commande à pas avance
  d'un pas pour chaque largeur de pas de rotation, et un pas n'est repris qu'après une largeur de pas entière dans
  l'autre sens, si bien qu'une main qui tremble ne la fait pas scintiller. Sans cible, un pincement affiche « Cliquez
  d'abord sur un bouton ».
- **La caméra comme contrôleur MIDI.** Réglages > Caméra > **Mode** choisit ce que font les mains : **Régler le bouton
  cliqué** (par défaut, ci-dessus), **Contrôleur MIDI** ou **Les deux**. Dans le mode Contrôleur MIDI, la caméra est une
  entrée nommée **Caméra** que l'apprentissage MIDI peut assigner à tout ce qu'il sait assigner aujourd'hui (un
  paramètre d'appareil ou de plugin, une commande de la table de mixage, le tempo, une touche de transport ou une
  commande) : armez une commande comme d'habitude, puis bougez la main. Ses messages ne quittent jamais l'application ;
  ils prennent le même chemin qu'un contrôleur matériel (apprentissage MIDI, liste des assignations MIDI, macros,
  modulation par une assignation). Une assignation apprise depuis la caméra est enregistrée comme n'importe quelle
  autre, s'affiche « Caméra, main droite rotation » avec son message dessous, et n'écoute que la caméra : un contrôleur
  matériel qui envoie le même numéro sur le même canal ne la pilote pas, et la caméra ne pilote pas une assignation qui
  écoute un port matériel ou n'importe quel port. Les commandes sont fixes : la main droite envoie sur le canal 1 et la
  main gauche sur le canal 2 (l'aperçu montre la main et son canal).

  | Commande | Message |
  | --- | --- |
  | Rotation en pinçant | Encodeur relatif, contrôleur 20 (complément à deux), un bouton sans fin avec un cliquet |
  | Pincement | Note 60, activée quand les doigts se touchent et désactivée quand ils s'ouvrent |
  | Hauteur de la paume | Contrôleur 21, 0 à 127 sur les 80 % centraux de l'image, le haut vaut 127 |
  | Paume à gauche et à droite | Contrôleur 22, même plage ; déplacer la main vers la droite (dans l'aperçu) l'augmente |
  | Proximité de la main | Contrôleur 23, d'une main éloignée (0) à une main proche de la caméra (127) |
  | Paume ouverte | Note 61, les quatre doigts écartés et le pouce dégagé de l'index pendant 300 ms |
  | Poing | Note 62, les quatre doigts repliés pendant 300 ms |
  | Pouce levé | Note 63, un poing dont le pouce se dresse au-dessus des articulations pendant 300 ms |
  | Index levé | Note 64, l'index levé et les trois autres doigts repliés pendant 300 ms |
  | Paix | Note 65, l'index et le majeur levés et les deux autres repliés pendant 300 ms |
  | Balayage à gauche, balayage à droite | Notes 66 et 67, activées et désactivées aussitôt : la paume balaie une demi-hauteur d'image de côté en 400 ms (gauche et droite comme l'aperçu les montre) |
  | Pad à deux mains | Contrôleurs 24 (gauche et droite), 25 (hauteur) et 26 (écartement) sur le **canal 3**, tant que les deux mains pincent |

  Une rotation apprise devient un encodeur sans fin (un tour complet du réglage de rotation balaie toute la plage d'une
  assignation au pas d'encodeur par défaut). Les contrôleurs n'envoient que lorsque leur valeur bouge d'un pas de 1 ou
  plus, au plus 60 fois par seconde chacun, à partir d'une position de paume lissée ; quand la main est perdue, ils
  gardent leur dernière valeur (pas de saut à 0) et les notes s'achèvent. **Les deux** tourne le bouton cliqué avec le
  pincement et la rotation et envoie les autres commandes ; le contrôleur de rotation n'est alors pas envoyé, de sorte
  qu'un seul geste ne pilote jamais deux cibles. Avec l'apprentissage MIDI armé (mode Apprentissage, ou APPRENDRE MIDI
  dans une fenêtre de plugin), la caméra envoie toutes ses commandes quel que soit le mode, un pincement ne tourne aucun
  bouton, et l'aperçu le dit, pour que vous puissiez apprendre une commande sans changer de mode. Dans le mode
  Contrôleur MIDI, l'aperçu montre six petites barres avec les valeurs en direct (rotation, hauteur, gauche et droite,
  proximité, pincement et paume).
- **Le pad à deux mains.** Pincez avec les deux mains : le milieu entre les mains est une position X/Y et la distance
  entre elles un troisième axe. Dans le mode **Régler le bouton cliqué**, le pad tourne les **deux** derniers boutons
  sur lesquels vous avez cliqué, le plus ancien avec le mouvement gauche-droite et le plus récent avec la hauteur,
  chacun à partir de là où il était quand vous avez pincé (déplacer les mains sur 80 % de l'image balaie une plage
  entière), de sorte que vous pouvez balayer ensemble la coupure et la résonance d'un filtre. Tout le geste à deux
  mains est une étape d'annulation. Avec moins de deux boutons cliqués, l'indicateur affiche « Cliquez d'abord sur deux
  boutons ». Dans le mode **Contrôleur MIDI**, le pad envoie les contrôleurs 24 à 26 sur le canal 3 (« les deux mains »,
  dans le tableau ci-dessus) : en absolu, au plus 60 fois par seconde chacun, gardant leurs dernières valeurs quand vous
  relâchez. Dans le mode **Les deux**, le pad tourne les boutons et n'envoie que son écartement. L'aperçu montre un
  réticule X/Y, avec un anneau aussi large que la distance entre vos mains, tant que le pad est actif.
- **Les gestes qui lancent des commandes.** La caméra connaît un petit ensemble fixe de gestes : **poing** (tous les
  doigts repliés), **paume ouverte**, **pouce levé**, **index levé** (l'index levé), **paix** (index et majeur levés) et
  **balayage à gauche ou à droite**. Réglages > Caméra > **Commandes de gestes** dit quelle commande chacun lance, pour
  l'une ou l'autre main, la gauche ou la droite : choisissez n'importe quelle commande dans la liste (elle a un champ de
  recherche, comme les raccourcis clavier), ou Aucune. **Le tableau est vide par défaut, rien ne se passe donc par
  surprise.** **Suggestion** remplit poing = Arrêt, paume ouverte tenue une seconde = Lecture et pouce levé =
  Enregistrer ; **Effacer** le vide. Un geste lance sa commande une fois à chaque fois que vous le faites (tenez la
  pose 300 ms ; la paume ouverte une seconde) et une nouvelle fois seulement après que votre main a changé de pose ;
  aucun geste ne se lance dans la demi-seconde qui suit une autre commande, et aucun ne se lance pendant que vous
  pincez. Une commande se lance exactement comme le ferait la touche qui lui est liée, avec les mêmes confirmations, et
  une commande qui ne s'applique pas dans la vue en cours le dit dans la ligne d'état. Les gestes arrivent aussi sous
  forme de notes (62 à 67, dans le tableau ci-dessus), l'apprentissage MIDI peut donc aussi les assigner.
- **Pour ce projet seulement.** Dans Réglages > Caméra, **À utiliser pour ce projet seulement** garde le mode et le
  tableau de gestes dans le projet ouvert au lieu de vos réglages : tant qu'il est activé, ces deux réglages modifient
  la copie du projet, les autres projets et vos réglages ne sont pas touchés, et le désactiver revient à vos réglages.
  Il est enregistré avec le projet (un champ facultatif, si bien que les versions plus anciennes ouvrent le projet et
  l'ignorent) et c'est une étape d'annulation.
- **Notes et pistes.** La caméra est un contrôleur, pas un instrument : une piste de notes qui écoute toutes les
  entrées ne reçoit jamais les notes de pincement et de paume ouverte, et vous pouvez les assigner à un interrupteur ou
  à une commande. Pour jouer avec elles volontairement, activez **Les notes de la caméra jouent l'instrument
  sélectionné** dans Réglages > Caméra : elles jouent alors l'instrument sélectionné (et enregistrent sur une piste
  armée) comme un clavier, sauf si une assignation qui consomme la note l'a prise. Les contrôleurs s'enregistrent comme
  automation en les assignant à un paramètre et en enregistrant la prise comme d'habitude.
- **L'aperçu.** Une petite boîte dans le coin inférieur droit de la fenêtre principale (Affichage > Panneaux > Aperçu
  de la caméra, ou son bouton de fermeture, `camera.preview`) : l'image de la caméra montrée comme un miroir avec les
  repères de la main dessus, l'état (PAS DE MAIN, MAIN ou PINCE), la cible et sa valeur pendant que vous pincez, et une
  info-bulle avec les images par seconde et les millisecondes que les modèles ont prises.
- **Le calibrage.** Si le bouton tourne trop peu, trop ou dans le mauvais sens, calibrez : Réglages > Caméra >
  **Calibrer**, Affichage > Panneaux > Calibrer la rotation de la caméra ou `camera.calibrate`. La caméra et l'aperçu
  s'allument et la boîte d'aperçu dit chaque étape : pincez et gardez la main immobile (1,5 seconde), continuez de
  pincer et tournez à droite aussi loin que c'est confortable puis tenez, ensuite tournez à gauche et tenez. L'écart
  entre vos deux extrémités devient **Rotation pour toute la plage**, le tremblement de votre main immobile fixe la
  zone morte (une rotation plus petite est ignorée, si bien qu'une main immobile ne dérive pas), et une rotation à
  droite que la caméra lit comme une rotation à gauche active **Inverser le sens de rotation**. Relâcher au milieu
  recommence ; la commande de nouveau annule ; rien ne change si elle ne se termine pas en 30 secondes. Pendant qu'il
  tourne, un pincement ne tourne aucun bouton.
- **Réglages.** L'onglet Caméra : la caméra (la liste est demandée avec Actualiser, ouvrir les réglages ne touche donc
  jamais une caméra), quelle main (l'une ou l'autre, la gauche ou la droite), le tour complet, **Inverser le sens de
  rotation**, **Calibrage de la rotation** (la zone morte et le bouton Calibrer), le réglage fin, **Détection du
  pincement** (à quel point un pincement peut être lâche ; elle ne change pas la rotation), l'aperçu, et les commandes
  de gestes. Ils sont enregistrés avec les autres réglages (`workflow.json`), sauf ce que **À utiliser pour ce projet
  seulement** garde dans le projet.
- **Confidentialité.** Les images ne vivent qu'en mémoire : elles ne sont jamais écrites sur le disque, journalisées,
  envoyées où que ce soit, mises dans un projet ni gardées après l'arrêt de la caméra, et un point allumé s'affiche
  chaque fois que la caméra tourne. Voir [Confidentialité](#privacy).

## Contrôleurs {#controllers}

Réglages > Contrôleurs > Configurer le contrôleur... apprend un contrôleur matériel d'après les messages qu'il envoie :
bougez chaque bouton et chaque fader, appuyez sur chaque touche, et passez ce qui manque au contrôleur. L'assistant
demande huit boutons, huit faders, le fader master, Lecture, Arrêt, Enregistrer, Boucle, Banque gauche, Banque droite
et, en dernier, Page précédente et Page suivante. Si le contrôleur a une sortie MIDI, l'assistant demande aussi si ses
boutons, faders et touches renvoient des valeurs (anneaux de LED, moteurs, touches allumées). Le résultat est un profil,
mémorisé sous le nom de port du contrôleur, et le contrôleur fonctionne la fois suivante où on le branche.

- **Boutons et pages.** Les huit boutons pilotent huit paramètres de l'appareil sélectionné (ou de l'appareil auquel la
  surface est verrouillée), dans l'ordre du panneau de l'appareil, sans l'interrupteur de contournement. La page 1 est
  formée des paramètres 1 à 8, la page 2 des paramètres 9 à 16, et ainsi de suite aussi loin que va l'appareil ; après
  la dernière page, on revient à la première. Page suivante montre la page suivante et Page précédente la précédente.
  Les deux sont facultatives dans l'assistant. Les commandes `controller.knob_page_next` et
  `controller.knob_page_prev` font la même chose et n'ont pas de touche par défaut (donnez-leur-en une dans
  Réglages > Clavier). La ligne d'état nomme la page quand elle change (« Knobs: Filter 9-16 »). La page revient à 1
  quand un autre appareil est sélectionné, sauf si le profil garde une page pour chaque appareil (voir plus bas). Un
  contrôleur à moteur ou à anneau de LED reçoit aussitôt les valeurs de la nouvelle page.
- **Reprendre les valeurs.** Quand un bouton ou un fader n'est pas là où est son paramètre (vous avez sélectionné un
  autre appareil, changé de page ou de banque, ou déplacé le paramètre à la souris), le bouger ne fait rien tant qu'il
  n'est pas à moins de 2 pour cent de la valeur ou ne la traverse pas, puis il reprend la main, si bien que le son ne
  saute jamais. Pendant qu'une commande attend, la ligne d'état dit dans quel sens la tourner (« Turn knob 3 up to pick
  up Cutoff »). C'est la même règle que l'apprentissage MIDI. Un encodeur relatif n'a rien à rattraper, un paramètre
  qui passe par des choix est réglé aussitôt, et un fader qui reçoit sa valeur en retour (un fader motorisé) est déjà en
  place, si bien qu'aucun d'eux n'attend.
- **Les deux options.** Reprendre les valeurs (activée pour un nouveau profil) et Garder la page de boutons par appareil
  (désactivée pour un nouveau profil, activée elle retient la page de chaque appareil pour la session) sont des cases à
  cocher de la dernière page de l'assistant et des pastilles dans Réglages > Contrôleurs tant que la surface fait
  tourner un profil. Elles sont enregistrées dans le fichier du profil ; un profil d'une version antérieure se lit avec
  les valeurs par défaut.

## Ducking {#ducking}

Pour qu'une piste s'efface sous une autre (une basse sous une grosse caisse, un pad sous une voix), faites un clic droit
sur l'en-tête de la piste et choisissez **Ducking depuis**, puis la piste à écouter. Le même sous-menu se trouve dans le
menu du clic droit de l'appareil de la piste dans le rack et dans le menu de tranche de la table de mixage (Ducking
depuis...), et dans Piste > Sidechain rapide.

- **Ce que cela fait.** Un Compresseur nommé « Duck from Kick » est ajouté à la fin des effets d'insert de la piste
  qui s'efface. Sa clé (l'entrée de sidechain) est la sortie de la Kick après son fader, si bien que la basse plonge
  quand la grosse caisse est mixée. Il est réglé sur un rapport de 4:1, 5 ms d'attaque, 120 ms de relâchement et un
  seuil de -24 dB. Le message dit quel bouton tourner : Seuil sur le Compresseur. Un seuil plus bas creuse plus profond,
  et un relâchement plus long laisse revenir la basse plus tard. Une annulation retire le compresseur et le câble de
  clé.
- **Pré-fader.** Ducking depuis (pré-fader) prend la clé avant le fader et le pan de la source (sa sortie parallèle),
  si bien que le ducking ne change pas quand vous baissez la source. Une source en mute ne donne pas de clé non plus,
  dans les deux cas.
- **La liste.** Les autres pistes sont listées par nom, chacune avec sa couleur ; les dossiers sont aplatis. Une piste
  qui est déjà une source est marquée « (ducking) ». Au-delà de trente pistes, la liste s'arrête et Plus... ouvre une
  fenêtre avec toutes les pistes.
- **De nouveau.** Faire un ducking depuis une piste dont la piste fait déjà le ducking n'ajoute rien : le compresseur
  est sélectionné dans le rack et le message dit « Already ducking from Kick ». Un ducking depuis plusieurs pistes
  ajoute un compresseur pour chacune.
- **Boucles.** Une piste ne peut pas faire de ducking depuis une piste qu'elle alimente, comme un bus dans lequel elle
  joue : la clé reboucherait. La ligne d'état le dit et rien ne change.
- **Supprimer le ducking.** Retire tous les compresseurs faits de cette façon (ils sont retrouvés par leur nom et leur
  câble de clé, renommer une piste ne casse donc rien) et les câbles de clé, en une étape. Renommer le compresseur pour
  qu'il ne commence plus par « Duck from » en fait un insert ordinaire que Supprimer le ducking laisse tranquille.
- **Pompage doux.** Une autre façon, plus légère, de faire respirer une piste : un outil Sidechain en mode pompage sur
  des noires, sans clé et sans autre piste. Il suit la position du morceau et le tempo.

## Écran de démarrage et modèles de genre {#start-screen-and-genre-templates}

L'écran de démarrage (Fichier > Écran de démarrage, et au lancement) liste Nouveau, Ouvrir, les modèles et les morceaux
récents. Les six premiers modèles sont livrés avec le programme : **Ambient**, **Hip-Hop Boom Bap**, **House**,
**Lo-fi**, **Synthwave** et **Techno**. Pointez-en un pour voir sa fiche (tempo, mesure et pistes), cliquez dessus pour
démarrer un nouveau morceau à partir de lui. Fichier > Nouveau depuis un modèle liste les mêmes modèles, suivis des
vôtres (seize au plus en tout). Un modèle s'ouvre comme un morceau sans titre, si bien qu'Enregistrer demande un
emplacement et que le modèle lui-même n'est jamais modifié.

- **Ce qu'il y a dedans.** Un tempo et une tonalité du morceau, huit sections sur une grille de 8 mesures sur 64
  mesures (Intro, Couplet ou Montée, Refrain ou Drop, Pause, Outro), montrées comme des plages dans l'arrangement. Six
  pistes nommées et colorées : une grosse caisse et le reste de la batterie (la boîte à rythmes avec les pads d'un kit
  d'usine), une basse, des accords ou un pad, un lead ou un arpège et une texture. Leurs clips sont les clips d'usine,
  ou de courts motifs dérivés d'eux, disposés pour que le morceau joue de la première à la dernière mesure, avec
  quelques sections laissées nues pour vos propres parties.
- **Le lanceur.** Quatre ou cinq scènes contiennent les boucles principales (intro, groove, accroche, pause), si bien
  que le lanceur est plein la première fois que vous l'ouvrez avec F10.
- **Le mixage.** Chaque piste se place près de -18 dBFS RMS (le gain d'entrée du canal règle le niveau, les faders
  restent près de l'unité). La basse a l'égaliseur de tranche activé et fait un ducking depuis la grosse caisse
  (Ducking depuis, voir Ducking). Deux envois alimentent une réverbération d'espace et un écho. Le master a un
  compresseur de colle et un limiteur avec un plafond de -1 dBTP, réglés pour que le morceau atterrisse près de
  -14 LUFS.
- **Les informations du morceau.** Fichier > Informations du morceau contient le titre, l'auteur (Atomik Rack factory)
  et une note sur la façon dont le modèle est construit et ce qu'il faut essayer.
- **Pas d'audio.** Un modèle n'utilise que les appareils du programme, il est donc petit et joue sur n'importe quelle
  machine.

## Partager un projet {#share-a-project}

Fichier > Partager le projet (dans le sous-menu Fichiers du projet, à côté d'Exporter l'archive du projet) envoie un
morceau à un ami ou à un collaborateur sous la forme d'un seul fichier qui s'ouvre partout. Le panneau a le **Titre**,
l'**Auteur** et une **Note** (ce sont les informations du morceau : PARTAGER les y écrit, en une étape d'annulation,
quand vous les avez changées) et trois cases à cocher :

- **Inclure un aperçu.** Une copie d'écoute de tout le morceau à -14 LUFS, faite par le même chemin qu'Exporter le
  morceau en fichier audio : un MP3 quand FFmpeg est configuré (Réglages > Vidéo), un fichier WAV 16 bits sinon. La
  ligne qui suit la case dit lequel.
- **Inclure les stems.** Un fichier WAV 24 bits par tranche de la table de mixage, issu de l'export des stems. Cela
  alourdit beaucoup le partage.
- **Inclure la liste des plugins.** Les plugins que le morceau utilise, avec leur éditeur et leur format, dans les
  crédits.

La ligne sous les cases dit ce que pèsera le partage (« This share will be about 12.3 MB ») ; elle prend la couleur
d'avertissement au-dessus de 500 Mo, et le message au démarrage du partage donne d'abord la taille. PARTAGER demande un
nom de fichier (`<titre>.atomikz`) et écrit le fichier sur un thread de travail, avec la progression dans la ligne
d'état et **Fichier > Fichiers du projet > Annuler l'archive du projet** pour l'arrêter ; une annulation ou un échec ne
laisse aucun fichier derrière lui.

Le fichier est une archive de projet (le même ZIP stocké qu'Exporter l'archive du projet, si bien que n'importe quel
outil ZIP l'ouvre) avec davantage dedans : le projet et chaque fichier audio qu'il utilise (pris dans le dossier du
projet, si bien que le destinataire n'a besoin ni de votre dossier audio ni de votre version de la bibliothèque
d'usine), `credits.txt` (titre, auteur, note, la date, la version du programme et les plugins avec éditeur et format,
pour que le destinataire sache ce qui manque), `preview.mp3` ou `preview.wav`, et le dossier `stems` quand il est
demandé.

**Ouvrir un partage.** Fichier > Ouvrir (ou un dépôt sur la fenêtre) accepte un fichier `.atomikz` : une boîte de
dialogue de dossier demande où le dézipper, en commençant dans `Shared` à côté du dossier de la bibliothèque (par
défaut `Documents/Atomik Rack/Shared`), le projet va dans un dossier de son propre nom (`Nom 2` quand celui-là existe ;
rien de ce qui existe n'est touché), s'ouvre après la question sur les modifications non enregistrées, et le panneau
Informations du morceau montre le titre, l'auteur et la note qu'a écrits l'expéditeur. `credits.txt` est dans le
dossier. Un plugin dont le morceau a besoin et que vous n'avez pas est listé comme l'est toujours un plugin manquant, et
le morceau joue avec un remplaçant. Une archive endommagée, une archive dont des entrées sortent de son dossier ou
qu'aucun partage ne contient, ou une archive qui dépasse les limites de taille, est refusée avant que rien ne soit
écrit.

## Copies de sauvegarde {#backup-copies}

En plus du fichier de projet, de ses versions et des instantanés de l'enregistrement automatique, Atomik Rack peut
garder une copie de sauvegarde de chaque projet enregistré dans un second dossier, par exemple sur un autre disque ou
dans un dossier que vous synchronisez ailleurs. C'est pour le jour où un disque tombe en panne. Réglages > Sauvegarde a
trois commandes : le **Dossier de sauvegarde** (Aucun tant que vous n'en choisissez pas un, avec **Choisir...** et
**Aucun**), **Garder** (combien de copies de chaque projet restent, 1 à 50, 10 par défaut ; Maj le change de dix) et
**Inclure l'audio** (activé par défaut).

Chaque enregistrement réussi, Fichier > Enregistrer, Enregistrer sous, Rassembler les fichiers et Déplacer le projet
aussi, copie alors le projet dans `<dossier de sauvegarde>/<nom du projet>/<nom du projet> <date et heure>.atomik`
(l'heure est en UTC, à la seconde, comme `Song 2026-11-10 093112.atomik`). Avec Inclure l'audio, les fichiers audio
qu'utilise le projet vont dans `<dossier de sauvegarde>/<nom du projet>/Audio/`. L'audio est nommé d'après son
contenu, si bien qu'un fichier n'est copié qu'une fois et que les enregistrements suivants l'y trouvent. La copie
tourne sur son propre thread de travail une fois l'enregistrement terminé : elle ne retarde jamais un enregistrement et
n'en fait jamais échouer un. Les instantanés de l'enregistrement automatique servent à la récupération après un plantage
et ne font pas de sauvegarde.

Quand il y a plus de copies que **Garder**, les plus anciennes sont supprimées. Un fichier audio ne part que si aucune
copie qui reste ne le nomme : chaque fichier de projet du dossier de sauvegarde du projet est lu d'abord, et si l'un
d'eux ne peut pas être lu, aucun audio n'est supprimé du tout. Les fichiers que vous y avez mis vous-même et qui ne sont
pas des copies de ce projet ne sont jamais touchés. Deux projets du même nom partagent un seul dossier dans le dossier
de sauvegarde, donnez-leur donc des noms différents.

Si une copie échoue (le dossier a disparu, le disque est plein), la ligne d'état dit `Backup failed: <la raison>` une
fois par session et tout échec suivant va seulement dans le journal ; l'enregistrement lui-même est bon et le suivant
réessaie.

**Fichier > Restaurer une sauvegarde...** liste les copies du projet ouvert, la plus récente d'abord, avec la date et la
taille de chacune. Choisissez-en une (Haut et Bas, un clic, puis RESTAURER ou Entrée ; un double-clic restaure aussitôt)
et elle s'ouvre comme un nouveau morceau non enregistré, comme un modèle : ce n'est jamais le fichier du morceau, si
bien que ni la copie ni le projet ne sont écrasés, et Enregistrer demande où la garder. La question sur les
modifications non enregistrées vient d'abord. Son audio vient du dossier `Audio` propre à la copie (une copie faite sans
Inclure l'audio s'ouvre avec son audio listé comme manquant, à relier de nouveau).

## Thèmes {#themes}

Atomik Rack a trois thèmes : **Sombre** (celui qu'il a toujours eu), **Clair**, et **Synthwave**, une nuit néon avec un
fond indigo profond, des accents magenta vif et cyan électrique, une grille magenta dans l'arrangement et des vumètres
qui passent du cyan au magenta puis à l'orange. **Réglages > Thèmes** les liste avec une rangée d'échantillons de
couleur chacun ; un clic sur l'un l'utilise aussitôt, dans chaque fenêtre et chaque panneau, et il est gardé pour le
prochain démarrage. Affichage > Thème a les mêmes choix (Sombre, Clair, Comme le système) et **Thème suivant**, qui
parcourt toute la liste, et **Gestionnaire de thèmes...** ouvre l'onglet. La rangée Thème de Réglages > Interface
montre le thème utilisé ; la faire défiler ramène à Sombre, Clair et Comme le système.

Vos propres thèmes sont des fichiers. Sous la liste :

- **Dupliquer** fait votre propre copie de n'importe quel thème, nommée « <nom> copy ». Sombre, Clair et Synthwave ne
  peuvent être ni modifiés ni supprimés, une copie est donc la façon de commencer.
- **Renommer** saisit un nouveau nom dans la rangée (Entrée le garde, Échap laisse l'ancien). Deux thèmes ne peuvent pas
  porter le même nom.
- **Supprimer** demande d'abord. Si le thème était utilisé, le choix revient à Sombre, Clair ou Comme le système.
- **Modifier...** ouvre l'éditeur de thème sur l'un de vos thèmes.
- **Importer...** prend un fichier de thème (`.json`), le vérifie et le copie dans le dossier des thèmes (avec un nombre
  après le nom si le nom est pris). **Exporter...** enregistre le thème sélectionné dans un fichier `.json` que vous
  pouvez partager.
- **Ouvrir le dossier des thèmes** montre le dossier dans le gestionnaire de fichiers : le dossier `themes` à côté de
  `settings.json`. Un thème est un fichier `.json` à cet endroit, et vous pouvez le modifier avec n'importe quel
  éditeur de texte (docs/EXTENDING.md, « Making a theme », donne le format).

Un fichier qui n'est pas un thème (ce n'est pas du JSON, il a une couleur qui n'est pas `#rrggbb`, ou c'est une version
plus récente du format) est ignoré au démarrage du programme, avec un message qui liste les fichiers et une ligne dans
le journal ; le fichier n'est jamais touché. Un thème qui omet une couleur la prend dans le thème intégré dont il part,
Clair ou Sombre.

### L'éditeur de thème {#the-theme-editor}

L'éditeur liste chaque couleur du thème par groupes (Arrière-plans, Texte, Accents, Vumètres, Arrangement, Rack et
Console). Chaque rangée a un échantillon et la couleur sous forme de texte :

- Cliquez sur l'**échantillon** pour ouvrir le sélecteur de couleur ; **Appliquer** donne la couleur à cette rangée.
- Cliquez sur le **texte hexadécimal** et tapez une couleur, `#rrggbb` ou `#rrggbbaa` quand la couleur est
  transparente. La rangée change dès que le texte est une couleur.
- **RÀZ** (montré sur une rangée qui diffère du thème dont elle part) remet la couleur à celle du thème de base.
- **ANNULER**, ou Ctrl+Z (Cmd+Z sur macOS), reprend le dernier changement ; l'historique dure jusqu'à la fermeture de
  l'éditeur.
- Haut et Bas passent à la rangée suivante, **ENREGISTRER** (Ctrl+S) écrit le fichier, et **FERMER** (Échap) l'écrit
  aussi quand quelque chose a changé.

Chaque changement se voit aussitôt dans tout le programme derrière l'éditeur, y compris dans la partie du thème
utilisé. Quand l'éditeur se ferme, le programme revient au thème utilisé. Les jetons de couleur :

| Groupe | Jetons |
| --- | --- |
| Arrière-plans | `background`, `rail`, `rail_hole`, `ear`, `panel_top`, `panel_bottom`, `back_top`, `back_bottom`, `plate`, `lcd`, `button`, `browser_bg`, `browser_row`, `tooltip_bg`, `error_bg`, `bevel_light`, `bevel_dark`, `section_line`, `ink`, `veil`, `dim` |
| Texte | `text`, `text_muted`, `on_accent`, `on_dark`, `on_error` |
| Accents | `highlight`, `focus`, `selection`, `warning`, `mute` |
| Vumètres | `meter_low`, `meter_mid`, `meter_high` |
| Arrangement | `lane`, `lane_alt`, `lane_auto`, `grid_bar`, `grid_beat`, `grid_sub`, `row_black`, `key_white`, `key_black` |
| Rack | `screw_light`, `screw_dark`, `knob_top`, `knob_bottom`, `knob_track`, `knob_pointer`, `jack_light`, `jack_dark`, `jack_hole`, `led_off`, `cable_audio`, `cable_cv`, `cable_notes`, `cable_insert`, `cable_send`, `cable_sidechain`, `cable_shadow` |
| Console | `io_connected`, `io_free`, `io_unavailable`, `clip` |

Outre les couleurs, un fichier de thème peut régler `dark` (texte clair sur surfaces sombres), `accent_saturation` et
`accent_lightness` (les couleurs que reçoivent les appareils et les pistes quand rien ne nomme les leurs ; celles de
Synthwave sont néon) et `corner` (le rayon des coins de panneau). Gardez le texte lisible : les thèmes intégrés gardent
leur texte courant à un contraste d'au moins 4,5 pour 1 contre chaque panneau, et un thème que vous faites n'est pas
vérifié.

Le thème utilisé est une préférence, pas une partie d'un projet : les morceaux ont le même aspect dans chaque thème, et
rien du thème n'y est enregistré.

## Charge CPU {#cpu-use}

Le programme chronomètre en permanence chaque appareil du rack sur le thread audio ; le coût est faible (environ un
demi pour cent d'un rappel de 64 images pour cent appareils) et il ne change jamais le son. Ce qu'il montre est la part
du temps audio qu'un appareil utilise : 100 % signifie que l'appareil seul prend autant de temps que l'audio qu'il
produit.

- **Sur les appareils.** Affichage > Afficher la charge CPU des appareils (désactivé au départ, non enregistré) place un
  petit chiffre sur la plaque de nom de chaque appareil du rack, comme `3.2%` (une décimale sous 10 %, pourcentage
  entier au-dessus), moyenné sur une seconde environ. Un conteneur montre la somme des appareils qu'il renferme. Un
  chiffre au-dessus de 25 % prend la couleur d'avertissement du thème.
- **Sur la table de mixage.** Avec le même interrupteur activé, chaque tranche de canal montre le total de sa piste dans
  le coin supérieur droit de la plaque de nom : l'instrument, le canal et ses effets d'insert.
- **La ligne d'état.** Posez le pointeur sur la ligne d'état au pied de la fenêtre : l'info-bulle donne la charge du
  moteur et son pic, puis les cinq appareils les plus lourds avec la piste à laquelle chacun appartient et son propre
  pic (maintenu deux secondes). Un plugin dans son propre processus compte le temps que le programme attend après lui.
- **Le conseil.** Quand la charge du moteur reste au-dessus de 70 % pendant cinq secondes alors que le morceau joue, une
  carte au-dessus de la ligne d'état dit quelle piste utilise le plus de CPU, comme « Track Bass uses the most CPU
  (41%). Freeze it? ». Geler lance Geler la piste dessus (Dégeler l'annule) ; Pas maintenant ferme la carte. Chaque
  piste n'est suggérée qu'une fois au plus par morceau, et une piste gelée ou sans clips n'est jamais suggérée.

## Plugins {#plugins}

Options > Gestionnaire de plugins est l'endroit où trouver, organiser et réparer les plugins tiers (VST3 et CLAP).
L'activation des plugins, les dossiers d'analyse et le bac à sable restent dans Audio > Plugins.

- **Le tableau.** Une rangée pour chaque plugin que l'analyse a trouvé : nom, éditeur, format, catégorie (ce que le
  plugin dit être : instrument, effet ou effet de notes), état, fichier et heure de sa dernière analyse. Un fichier dont
  l'analyse a échoué est aussi une rangée, avec sa raison dans l'état. Cliquez sur un en-tête de colonne pour trier,
  encore pour inverser. La colonne des étoiles trie les favoris d'abord.
- **Recherche et filtres.** Le champ de recherche trouve un mot dans le nom, l'éditeur, le chemin ou un tag. Les quatre
  boutons à côté parcourent leurs choix à chaque clic : format (tous, VST3, CLAP), état (tous, OK, échoué, non pris en
  charge, en quarantaine, désactivé), catégorie et tag.
- **Choisir des rangées.** Un clic choisit une rangée, Ctrl (Cmd sur macOS) en bascule une, Maj choisit une plage et
  Ctrl+A toutes les rangées affichées. Les boutons agissent sur les rangées choisies.
- **Favoris.** Cliquez sur l'étoile d'une rangée, ou appuyez sur Espace dessus. Dans le navigateur d'appareils, les
  plugins étoilés viennent d'abord, dans un groupe Favoris et en haut de leur genre.
- **Tags.** Tapez un tag dans le champ Tag et appuyez sur Ajouter un tag (ou Entrée) : les plugins choisis le reçoivent.
  Les tags du premier plugin choisi sont listés à côté du champ ; cliquez sur l'un pour le retirer. Dans le navigateur
  d'appareils, tapez `#` et le début d'un tag dans le champ de recherche pour lister les plugins qui l'ont. Les tags
  sont les mêmes mots que dans l'explorateur de fichiers.
- **Désactiver.** Masque les plugins choisis du navigateur et des menus. Un projet qui utilise un plugin désactivé le
  charge toujours, et Activer fait réapparaître le plugin.
- **Quarantaine.** Un plugin dont le processus se termine, ou qui ne peut pas être chargé, trois fois de suite est mis
  en quarantaine : il est masqué comme un plugin désactivé et un message le dit une fois. Un processus que l'application
  a arrêté parce qu'il ne répondait plus depuis des secondes n'est pas un plantage et ne compte pas. Une instance qui
  se termine sans faute, ou un chargement qui réussit, remet le compte à zéro. Réessayer sort les plugins choisis de la
  quarantaine (et analyse de nouveau les fichiers échoués).
- **Apprendre dans la fenêtre du plugin.** Chaque fenêtre d'éditeur de plugin a une fine bande ancrée à son bord
  supérieur (dessous quand la fenêtre est en haut de l'écran) : le nom du plugin, APPRENDRE AUTOMATION, APPRENDRE MIDI,
  ce qui s'est passé en dernier et un bouton de fermeture. Choisissez un mode, puis bougez n'importe quelle commande
  dans l'interface propre du plugin. APPRENDRE AUTOMATION montre la voie d'automation de ce paramètre dans
  l'arrangement, en ajoutant une voie vide quand il n'en a pas ; APPRENDRE MIDI attend une commande matérielle et
  l'assigne à ce paramètre (la bande dit lequel : `Cutoff ← CC 21 ch 1`). Le mode reste actif pour la commande
  suivante ; cliquez sur le bouton allumé ou sur le bouton de fermeture, appuyez sur Échap dans la fenêtre principale,
  fermez l'éditeur ou activez l'apprentissage MIDI pour l'arrêter. Le menu du clic droit de l'appareil plugin et la
  palette de commandes ont Apprendre l'automation depuis la fenêtre du plugin et Apprendre le MIDI depuis la fenêtre du
  plugin. Là où la bande ne peut pas être affichée (une session Wayland), le panneau de paramètres (PARAMS) a les deux
  mêmes boutons, et un toucher sur l'une de ses rangées fonctionne de la même façon.
- **Réanalyser.** Tout réanalyser analyse de nouveau chaque dossier de plugins, Réanalyser la sélection seulement les
  fichiers des rangées choisies. L'analyse tourne en arrière-plan : le panneau montre où elle en est et le même bouton
  devient Annuler.
- **Afficher dans le dossier.** Ouvre le gestionnaire de fichiers sur le fichier de la première rangée choisie.
- **Où c'est gardé.** Les favoris, les tags, les plugins désactivés et la quarantaine sont enregistrés sous l'id propre
  du plugin dans `plugins_user.json` à côté du fichier de réglages, ils restent donc avec un plugin déplacé vers un
  autre dossier. Un fichier manquant ou endommagé se lit comme vide.

## Fenêtres {#windows}

Le rack et la table de mixage peuvent quitter la fenêtre principale (Affichage > Panneaux > Détacher le rack et
Détacher la table de mixage, ou en fermant leur fenêtre pour les remettre). La barre de menus principale est en haut de
chacune de ces fenêtres, la même barre que dans la fenêtre principale : les mêmes menus, les mêmes coches et les mêmes
éléments estompés, si bien qu'une commande est toujours à un clic de la fenêtre où vous travaillez.

- **Sur quel panneau agit une commande.** Une commande qui agit sur « le panneau qui a les touches » agit sur le panneau
  de la fenêtre dont vous avez utilisé la barre. Édition > Tout sélectionner, Copier, Couper, Coller, Dupliquer et
  Supprimer agissent sur les appareils du rack dans la fenêtre du rack et sur les tranches de la table de mixage dans la
  fenêtre de la table de mixage. Les raccourcis clavier fonctionnent dans chaque fenêtre comme avant.
- **Un seul menu ouvert.** Un seul menu est ouvert à la fois. Ouvrir un menu dans une fenêtre ferme celui qui est ouvert
  dans une autre, et un menu se ferme quand sa fenêtre perd le focus. Tant qu'un menu est ouvert, les flèches, Début,
  Fin, Entrée et Échap le commandent, dans quelque fenêtre qu'il soit ; il n'y a pas de raccourcis Alt ou par lettre
  pour les titres de menu.
- **Taille.** Une fenêtre avec la barre n'est jamais plus étroite que la barre. La taille et la place de la fenêtre sont
  enregistrées comme avant.
- **Pas de barre.** Les petites fenêtres (Égaliseur à spectre, Clavier de piano, Vidéo) et le navigateur détaché n'ont
  pas de barre de menus. Sur macOS, la barre de menus est celle du système, en haut de l'écran, et les fenêtres n'en
  dessinent aucune.

## Confidentialité {#privacy}

Atomik Rack n'envoie rien sur l'utilisateur ni sur l'ordinateur. En dehors des téléchargements uniques de fichiers de
modèles, que seul l'utilisateur lance après une boîte de dialogue (les modèles de la main et les modèles de stems, rien
n'est envoyé avec eux), sa seule requête réseau est la vérification facultative d'une version plus récente (Aide >
Rechercher des mises à jour, et une fois par jour quand l'utilisateur a répondu Oui à la question du premier
démarrage) : une requête HTTPS vers la page du projet pour la liste des versions, qui ne porte que le nom et la version
du programme comme User-Agent. Rien n'est téléchargé ni installé, et tant que la question n'a pas reçu de réponse rien
n'est envoyé. Le fonctionnement est décrit dans [ARCHITECTURE.md](../ARCHITECTURE.md#the-update-check) ; le choix est
dans [DECISIONS.md](../DECISIONS.md#s-the-update-check).

Autres documents :

- [../../README.md](../../README.md) : vue d'ensemble, fonctionnalités, instructions pour compiler et lancer.
- [../../CONTRIBUTING.md](../../CONTRIBUTING.md) : la politique de travail original, la signature DCO, le déroulement
  et les règles.
- [../../ROADMAP.md](../../ROADMAP.md) : ce qui est prévu (des intentions, pas des promesses).
- [../../CHANGELOG.md](../../CHANGELOG.md) : les notes de version.
- [../../examples/README.md](../../examples/README.md) : les projets et patchs d'exemple.
- [../../SECURITY.md](../../SECURITY.md) et [../../CODE_OF_CONDUCT.md](../../CODE_OF_CONDUCT.md),
  [../../SUPPORT.md](../../SUPPORT.md).
- [RELEASING.md](../RELEASING.md) : comment une version est préparée, étiquetée et construite.
