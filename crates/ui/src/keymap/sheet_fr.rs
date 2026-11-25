//! The French text of the keyboard cheat sheet: the prose around the generated tables
//! (`docs/fr/KEYBOARD.md`). The tables themselves are built from the registry in
//! [`super::sheet`], with the French command titles and key names of the interface catalogue.
//! Every heading names the anchor of its English counterpart (`{#id}`), so a link to a section of
//! the sheet resolves in both languages.

use super::sheet::Prose;

const INTRO: &str = "Les combinaisons sont affichées comme les menus les affichent. `Ctrl` est la touche Contrôle ; sur macOS le \
modificateur principal est `Cmd` (Commande) et Alt est `Opt` (Option), et Contrôle et Commande ne sont jamais \
interchangés. Les touches `Numpad` se distinguent des touches principales. Les lettres sont reconnues d'après le \
caractère qu'elles tapent, si bien qu'une disposition qui déplace une lettre déplace aussi le raccourci ; chaque \
assignation peut être changée dans Réglages, Clavier (enregistrée dans `keymap.json` à côté de `settings.json`, \
seulement vos changements). Tant qu'un champ de texte a le focus, seules les combinaisons avec le modificateur \
principal, les touches de fonction et Échap lancent des commandes. Là où une assignation d'un panneau recoupe une \
assignation globale, le panneau l'emporte dans ce panneau. Le panneau de l'explorateur de fichiers (Affichage, \
Explorateur de fichiers, `F9`) est le contexte `files` : tant qu'il a le focus, ses touches (flèches, Page préc. et \
Page suiv., Début et Fin, Entrée, Espace, Échap, Retour arrière, `F`, la recherche et les combinaisons de sélection \
totale) sont les commandes `files.*`, que le shell transmet au panneau comme ces touches, de sorte qu'elles l'emportent \
sur les assignations globales des mêmes combinaisons (Espace écoute un aperçu au lieu de lancer la lecture, Retour \
arrière remonte d'un dossier au lieu de supprimer) ; toute autre combinaison, `F9` et `F5` compris, se lance comme \
d'habitude. Tant que le champ de recherche ou de tag du panneau a le focus, seuls Échap, les touches de fonction et \
les combinaisons avec le modificateur principal lancent des commandes. La bande Touches de piano (Affichage, Clavier de \
piano, `F4`) joue le clavier de l'ordinateur tant qu'elle a le focus en mode Touches : c'est le contexte `keys`, où les \
lettres sont des notes (deux rangées à partir de `A`, `W`, `S`, `E`... jusqu'au fa au-dessus du do suivant) et `Z` et \
`X` changent l'octave, `1` à `0` règlent la vélocité, et Maj est la pédale de sustain (elle ne change pas la touche que \
désigne une lettre) ; une combinaison que le contexte n'assigne pas, `Espace` ou toute combinaison avec le \
modificateur principal compris, se lance comme d'habitude. L'éditeur de tablature (Affichage, Éditeurs, Éditeur de \
tablature) est le contexte `tab` : les chiffres `0` à `9` tapent la case sur le curseur (deux chiffres tapés à un \
instant d'intervalle font une seule case, `12`), les flèches déplacent le curseur (Gauche et Droite selon la valeur de \
note, Haut et Bas entre les cordes, `Ctrl+Gauche` et `Ctrl+Droite` par mesure), les lettres `W`, `H`, `Q`, `E`, `S`, \
`T` et `X` règlent la valeur de note de la ronde à la quadruple croche, `.` la pointe et `Y` en fait un triolet, `=` et \
`-` zooment, et Suppr ou Retour arrière supprime les notes sous le curseur ou dans la sélection. Maj avec les flèches \
sélectionne (Gauche et Droite des colonnes entières de chaque corde, Haut et Bas un rectangle), `Ctrl+A` sélectionne \
toutes les notes, `Ctrl+C`, `Ctrl+X` et `Ctrl+V` copient, coupent et collent avec cordes, cases et techniques, \
`Alt+Haut` et `Alt+Bas` déplacent les notes vers la corde suivante qui joue la même hauteur, `Ctrl+Haut` et \
`Ctrl+Bas` transposent d'un demi-ton (avec Maj d'une octave), et les techniques sont `Maj+H` hammer-on, `Maj+P` \
pull-off, `Maj+L` glissé legato, `Maj+S` glissé décalé, `B` tiré, `Maj+B` tiré puis relâché, `Alt+B` pré-tiré, `V` \
vibrato, `Maj+V` vibrato large, `N` harmonique naturelle, `Maj+N` harmonique artificielle, `M` étouffé (paume), `R` \
laisser sonner, `D` note morte, `G` note fantôme, `A` accent, `Maj+T` tapé et `Maj+C` effacer (chacune bascule sur la \
sélection, ou sur la note sous le curseur). Dans le panneau, elles l'emportent sur une assignation globale de la même \
combinaison (seul `.`, la touche du marqueur suivant, en recoupe une). Tant que le panneau instrument, accordage et \
capo ou le sélecteur de doigté est ouvert, il prend toutes les touches. Le panneau Grille d'accords (Affichage, \
Éditeurs, Grille d'accords) n'assigne aucune touche qui lui soit propre : Haut, Bas, Page préc., Page suiv., Début et \
Fin font défiler le texte, Gauche et Droite parcourent les doigtés du diagramme d'accord ouvert et Échap le ferme. Ses \
commandes (transposer, capodastre, dièses ou bémols, taille du texte, défilement automatique, nouvelle, renommer, \
supprimer, modifier en texte et appliquer la transposition) n'ont pas de touche par défaut et se trouvent dans la \
palette de commandes. Tant que son texte, sa boîte de renommage ou sa liste d'instruments est ouvert, il prend toutes \
les touches (Ctrl+Entrée applique le texte, Échap le quitte). Un menu ouvert (un menu contextuel, ou un menu de la \
barre de menus dans la fenêtre) prend toutes les touches : Haut et Bas déplacent la surbrillance (elle reboucle, et \
saute les séparateurs et les éléments désactivés), Début et Fin vont au premier et au dernier élément, Droite ou \
Entrée ouvre un sous-menu sur son premier élément, Gauche ou Échap le ferme (Échap sur le premier panneau ferme le \
menu), Entrée ou Espace choisit, et dans la barre de menus Gauche et Droite passent au menu voisin (Droite sur un \
élément aussi).\n\n";

const POINTER: &str = "## Gestes du pointeur : tête de lecture, repères et marqueurs {#pointer-gestures-playhead-locators-and-markers}

| Geste | Où | Résultat |
|---|---|---|
| Appuyer et faire glisser | Règle de l'arrangement | Place la tête de lecture à la position aimantée sous le pointeur et continue de la placer tant que le pointeur bouge (jamais avant zéro) |
| Faire glisser la tête de lecture | Arrangement (le fanion de la règle, ou sa ligne dans la grille) et piano roll (sa ligne dans la grille, ou la règle) | Déplace la tête de lecture de la distance parcourue, aimantée |
| Faire glisser le fanion L ou R | Règle de l'arrangement, bande inférieure | Déplace le repère gauche ou le repère droit, aimanté, avec un pas d'aimantation laissé entre eux ; la boucle est réglée au relâchement (Échap annule) |
| `Ctrl+clic` (`Alt+clic` sur macOS) | Règle de l'arrangement | Place le repère gauche (le début de la boucle) à la position aimantée |
| `Alt+clic` (`Cmd+clic` sur macOS) | Règle de l'arrangement | Place le repère droit (la fin de la boucle) à la position aimantée |
| `Maj+clic` | Règle de l'arrangement | Place le marqueur de fin (End) à la position aimantée |
| Faire glisser une ligne de marqueur | Grille de l'arrangement, tant que la bande des marqueurs est affichée | Déplace le marqueur, comme le fait de faire glisser son étiquette dans la bande |
| `Ctrl+Alt` maintenu | Pendant le glissé de l'un de ces éléments | Pas d'aimantation |

Les clics de repère l'emportent sur le rasoir dans la règle. Dans la grille, la tête de lecture et les lignes de \
marqueur se saisissent à trois pixels de la ligne, avec l'outil de sélection, sans `Maj` ni `Ctrl` maintenus, et \
jamais par-dessus les commandes de bord, de fondu, de point ou de mute d'un clip ou d'une note ; ailleurs, l'appui fait \
ce qu'il faisait avant.

## Gestes du pointeur : valeurs {#pointer-gestures-values}

| Geste | Où | Résultat |
|---|---|---|
| Appuyer et faire glisser vers le haut ou le bas | Barre de transport, BPM | Change en direct le champ sous le pointeur, 5 pixels par pas : 1 BPM sur la partie entière (0,1 avec `Maj`), 0,01 sur les deux décimales ; tout le glissé est une étape d'annulation |
| Molette de la souris | Barre de transport, BPM | Un cran est un pas du champ sous le pointeur, comme pour le glissé |
| Appuyer et faire glisser vers le haut ou le bas | Barre de transport, mesure | Fait varier le nombre de temps par mesure (1 à 32) à gauche de la barre oblique ou l'unité de temps (2, 4, 8, 16) à droite ; une étape d'annulation par glissé |
| Molette de la souris | Barre de transport, mesure | Un cran est un pas du champ sous le pointeur |
| Appuyer et faire glisser vers le haut ou le bas | Sélecteur d'aimantation de l'arrangement et du piano roll | Parcourt les résolutions dans l'ordre du sélecteur, 5 pixels par pas, sans rebouclage ; un clic sans mouvement fait toujours avancer au relâchement, un clic droit reculer |
| Molette de la souris | Sélecteur d'aimantation de l'arrangement et du piano roll | Un cran est une résolution |
| Appuyer et faire glisser vers le haut ou le bas, molette de la souris | Une valeur des panneaux Quantifier, Audio vers notes, Supprimer les silences et Warp (chaque rangée avec un bouton - et +) | Fait varier la valeur, 5 pixels ou un cran par pas |
| Double-clic | Barre de transport, BPM ou mesure | Saisit la valeur (Entrée valide, Échap annule) |

Haut augmente et bas diminue, et une valeur s'arrête au bout de sa plage.

## Rôles des modificateurs {#modifier-roles}

Un rôle est un modificateur maintenu pendant un geste du pointeur, pas une combinaison. Réglages, Clavier liste les \
rôles sur lesquels un geste agit, et en réassigner un change aussitôt ce geste. Un rôle se distingue des autres de son \
geste par les modificateurs exacts maintenus (`Alt` seul et `Ctrl+Alt` sont deux prises différentes).

| Rôle | Autres systèmes | macOS | Résultat |
|---|---|---|---|
| Outil alternatif | `Alt` | `Ctrl` | Le pointeur agit avec l'outil alternatif tant qu'il est maintenu |
| Désactiver l'aimantation | `Ctrl+Alt` | `Cmd+Opt` | Pas d'aimantation pendant le glissé |
| Copier en faisant glisser | `Ctrl` | `Opt` | Faire glisser des clips dans l'arrangement dépose une copie |
| Basculer la sélection | `Ctrl` | `Cmd` | Un clic ajoute l'élément à la sélection ou l'en retire |
| Sélection de plage | `Maj` | `Maj` | Un clic sélectionne du dernier élément cliqué jusqu'à celui-ci |
| Réglage fin | `Maj` | `Maj` | Un glissé ou un cran de molette sur un bouton ou un fader du rack et de la table de mixage le déplace dix fois moins loin |
| Réinitialiser le paramètre | `Ctrl` | `Cmd` | Un clic sur un bouton ou un fader du rack et de la table de mixage le remet à sa valeur par défaut (un double-clic aussi) |
| Contraindre | `Maj` | `Maj` | Un glissé reste sur un seul axe ; le crayon sur un clip d'automation place ses points sur la droite à partir de l'appui |
| Plage d'automation plate | `Ctrl+Maj` | `Cmd+Maj` | Le crayon sur un clip d'automation tient la valeur où le tracé a commencé jusqu'au pointeur |
| Sélection de plage de voie | `Alt+Maj` | `Opt+Maj` | Un glissé sur une voie de l'arrangement sélectionne la plage de temps qu'il couvre (Copier, Couper, Supprimer et Coller agissent dessus) |
| Assigner à une macro | `Alt+Maj` | `Opt+Maj` | Un glissé d'un bouton ou d'un fader du rack vers un bouton de macro d'une banque de macros l'assigne à cette macro |
| Mise à l'échelle de la sélection | `Ctrl+Alt+Maj` | `Cmd+Opt+Maj` | Un glissé sur le bord de fin d'un clip sélectionné de l'arrangement met à l'échelle tous les clips sélectionnés autour du début de la sélection, un glissé sur le bord de début autour de sa fin (Alt seul met chaque clip à l'échelle autour de son propre bord fixe) |

Les endroits qui prennent d'eux-mêmes un pas plus fin avec le modificateur (l'inspecteur de clip, les panneaux de macros \
et de plugins, l'éditeur de conteneur) utilisent toujours `Maj`.

Dans la table de mixage, la molette change le bouton, le fader ou le bouton-poussoir sous le pointeur (un cran le \
déplace d'un pas ; avec Réglage fin maintenu, d'un dixième). Au-dessus de l'étiquette sous un bouton, au-dessus du fond \
d'une tranche ou au-dessus d'une barre de défilement, elle fait défiler les tranches : la molette fait défiler les \
sections du haut vers le haut et le bas, Maj (ou une molette latérale) les fait défiler de côté, et la molette au-dessus \
de la barre de défilement sous les tranches fait défiler de côté.

Beaucoup de dispositions de clavier européennes signalent AltGr comme `Ctrl+Alt` (la valeur par défaut de Désactiver \
l'aimantation hors macOS) : taper un caractère avec AltGr pendant un glissé désactive l'aimantation. Réassignez \
Désactiver l'aimantation dans Réglages, Clavier si cela gêne. Dans un champ de texte, AltGr tape toujours son caractère \
et ne lance jamais de raccourci.

## Champs de texte {#text-fields}

Chaque endroit où l'on tape du texte (noms, recherches, nombres et durées, commentaires) est le même champ. Un clic \
place le curseur, un glissé sélectionne, un double-clic sélectionne un mot et un triple-clic tout. Gauche et Droite \
déplacent d'un caractère, avec `Ctrl` (`Opt` sur macOS) d'un mot, `Début` et `Fin` vont aux extrémités de la ligne, et \
`Maj` avec l'une d'elles sélectionne. `Ctrl+A` sélectionne tout ; `Ctrl+C`, `Ctrl+X` et `Ctrl+V` copient, coupent et \
collent (`Cmd` sur macOS). `Ctrl+Z` annule dans le champ, `Ctrl+Maj+Z` (et `Ctrl+Y` hors macOS) rétablit ; une suite de \
lettres est une étape et chaque champ garde les 50 dernières. Tant qu'un champ a le clavier, ces combinaisons lui \
appartiennent et ne lancent jamais Annuler du morceau ; sans champ, `Ctrl+Z` est Annuler du morceau. Un clic droit sur \
un champ ouvre son propre menu (Annuler, Rétablir, Couper, Copier, Coller, Supprimer, Tout sélectionner). Les méthodes \
de saisie composent dans le champ. Entrée et Échap terminent la saisie comme le dit le panneau. Les champs qui prennent \
des nombres refusent les autres caractères.

## Info-bulles {#tooltips}

Laisser le pointeur sur un bouton environ 0,6 seconde affiche dessous une info-bulle d'une ligne qui dit ce que fait le \
bouton, avec le raccourci de sa commande quand il en a un (écrit comme le font les menus, si bien qu'une touche que vous \
réassignez dans Réglages, Clavier s'affiche aussitôt). L'info-bulle se masque à un appui, une touche, la molette, ou \
quand le pointeur quitte le bouton. Réglages, Workflow, **Afficher les info-bulles** (activé par défaut) les désactive ; \
la lecture de valeur affichée pendant qu'on fait glisser un bouton ou un fader reste.

";

pub(super) const FRENCH: Prose = Prose {
    title: "Référence du clavier d'Atomik Rack",
    generated: "<!-- Généré par `atomik --print-keymap --lang fr` ; ne pas modifier. Un test garde ce fichier identique à la sortie. -->",
    intro: INTRO,
    pointer: POINTER,
    where_: "Où",
    command: "Commande",
    moved_keys: "Touches déplacées : touches standard et touches historiques",
    unbound: "Commandes sans touche dans ce profil (les menus et les Réglages y mènent toujours)",
    profile_standard: "Touches standard",
    profile_legacy: "Touches historiques d'Atomik",
};
