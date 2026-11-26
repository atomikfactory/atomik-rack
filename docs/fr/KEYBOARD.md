# Référence du clavier d'Atomik Rack {#atomik-rack-keyboard-reference}

<!-- Généré par `atomik --print-keymap --lang fr` ; ne pas modifier. Un test garde ce fichier identique à la sortie. -->

Les combinaisons sont affichées comme les menus les affichent. `Ctrl` est la touche Contrôle ; sur macOS le modificateur principal est `Cmd` (Commande) et Alt est `Opt` (Option), et Contrôle et Commande ne sont jamais interchangés. Les touches `Numpad` se distinguent des touches principales. Les lettres sont reconnues d'après le caractère qu'elles tapent, si bien qu'une disposition qui déplace une lettre déplace aussi le raccourci ; chaque assignation peut être changée dans Réglages, Clavier (enregistrée dans `keymap.json` à côté de `settings.json`, seulement vos changements). Tant qu'un champ de texte a le focus, seules les combinaisons avec le modificateur principal, les touches de fonction et Échap lancent des commandes. Là où une assignation d'un panneau recoupe une assignation globale, le panneau l'emporte dans ce panneau. Le panneau de l'explorateur de fichiers (Affichage, Explorateur de fichiers, `F9`) est le contexte `files` : tant qu'il a le focus, ses touches (flèches, Page préc. et Page suiv., Début et Fin, Entrée, Espace, Échap, Retour arrière, `F`, la recherche et les combinaisons de sélection totale) sont les commandes `files.*`, que le shell transmet au panneau comme ces touches, de sorte qu'elles l'emportent sur les assignations globales des mêmes combinaisons (Espace écoute un aperçu au lieu de lancer la lecture, Retour arrière remonte d'un dossier au lieu de supprimer) ; toute autre combinaison, `F9` et `F5` compris, se lance comme d'habitude. Tant que le champ de recherche ou de tag du panneau a le focus, seuls Échap, les touches de fonction et les combinaisons avec le modificateur principal lancent des commandes. La bande Touches de piano (Affichage, Clavier de piano, `F4`) joue le clavier de l'ordinateur tant qu'elle a le focus en mode Touches : c'est le contexte `keys`, où les lettres sont des notes (deux rangées à partir de `A`, `W`, `S`, `E`... jusqu'au fa au-dessus du do suivant) et `Z` et `X` changent l'octave, `1` à `0` règlent la vélocité, et Maj est la pédale de sustain (elle ne change pas la touche que désigne une lettre) ; une combinaison que le contexte n'assigne pas, `Espace` ou toute combinaison avec le modificateur principal compris, se lance comme d'habitude. L'éditeur de tablature (Affichage, Éditeurs, Éditeur de tablature) est le contexte `tab` : les chiffres `0` à `9` tapent la case sur le curseur (deux chiffres tapés à un instant d'intervalle font une seule case, `12`), les flèches déplacent le curseur (Gauche et Droite selon la valeur de note, Haut et Bas entre les cordes, `Ctrl+Gauche` et `Ctrl+Droite` par mesure), les lettres `W`, `H`, `Q`, `E`, `S`, `T` et `X` règlent la valeur de note de la ronde à la quadruple croche, `.` la pointe et `Y` en fait un triolet, `=` et `-` zooment, et Suppr ou Retour arrière supprime les notes sous le curseur ou dans la sélection. Maj avec les flèches sélectionne (Gauche et Droite des colonnes entières de chaque corde, Haut et Bas un rectangle), `Ctrl+A` sélectionne toutes les notes, `Ctrl+C`, `Ctrl+X` et `Ctrl+V` copient, coupent et collent avec cordes, cases et techniques, `Alt+Haut` et `Alt+Bas` déplacent les notes vers la corde suivante qui joue la même hauteur, `Ctrl+Haut` et `Ctrl+Bas` transposent d'un demi-ton (avec Maj d'une octave), et les techniques sont `Maj+H` hammer-on, `Maj+P` pull-off, `Maj+L` glissé legato, `Maj+S` glissé décalé, `B` tiré, `Maj+B` tiré puis relâché, `Alt+B` pré-tiré, `V` vibrato, `Maj+V` vibrato large, `N` harmonique naturelle, `Maj+N` harmonique artificielle, `M` étouffé (paume), `R` laisser sonner, `D` note morte, `G` note fantôme, `A` accent, `Maj+T` tapé et `Maj+C` effacer (chacune bascule sur la sélection, ou sur la note sous le curseur). Dans le panneau, elles l'emportent sur une assignation globale de la même combinaison (seul `.`, la touche du marqueur suivant, en recoupe une). Tant que le panneau instrument, accordage et capo ou le sélecteur de doigté est ouvert, il prend toutes les touches. Le panneau Grille d'accords (Affichage, Éditeurs, Grille d'accords) n'assigne aucune touche qui lui soit propre : Haut, Bas, Page préc., Page suiv., Début et Fin font défiler le texte, Gauche et Droite parcourent les doigtés du diagramme d'accord ouvert et Échap le ferme. Ses commandes (transposer, capodastre, dièses ou bémols, taille du texte, défilement automatique, nouvelle, renommer, supprimer, modifier en texte et appliquer la transposition) n'ont pas de touche par défaut et se trouvent dans la palette de commandes. Tant que son texte, sa boîte de renommage ou sa liste d'instruments est ouvert, il prend toutes les touches (Ctrl+Entrée applique le texte, Échap le quitte). Un menu ouvert (un menu contextuel, ou un menu de la barre de menus dans la fenêtre) prend toutes les touches : Haut et Bas déplacent la surbrillance (elle reboucle, et saute les séparateurs et les éléments désactivés), Début et Fin vont au premier et au dernier élément, Droite ou Entrée ouvre un sous-menu sur son premier élément, Gauche ou Échap le ferme (Échap sur le premier panneau ferme le menu), Entrée ou Espace choisit, et dans la barre de menus Gauche et Droite passent au menu voisin (Droite sur un élément aussi).

## Gestes du pointeur : tête de lecture, repères et marqueurs {#pointer-gestures-playhead-locators-and-markers}

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

Les clics de repère l'emportent sur le rasoir dans la règle. Dans la grille, la tête de lecture et les lignes de marqueur se saisissent à trois pixels de la ligne, avec l'outil de sélection, sans `Maj` ni `Ctrl` maintenus, et jamais par-dessus les commandes de bord, de fondu, de point ou de mute d'un clip ou d'une note ; ailleurs, l'appui fait ce qu'il faisait avant.

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

Un rôle est un modificateur maintenu pendant un geste du pointeur, pas une combinaison. Réglages, Clavier liste les rôles sur lesquels un geste agit, et en réassigner un change aussitôt ce geste. Un rôle se distingue des autres de son geste par les modificateurs exacts maintenus (`Alt` seul et `Ctrl+Alt` sont deux prises différentes).

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

Les endroits qui prennent d'eux-mêmes un pas plus fin avec le modificateur (l'inspecteur de clip, les panneaux de macros et de plugins, l'éditeur de conteneur) utilisent toujours `Maj`.

Dans la table de mixage, la molette change le bouton, le fader ou le bouton-poussoir sous le pointeur (un cran le déplace d'un pas ; avec Réglage fin maintenu, d'un dixième). Au-dessus de l'étiquette sous un bouton, au-dessus du fond d'une tranche ou au-dessus d'une barre de défilement, elle fait défiler les tranches : la molette fait défiler les sections du haut vers le haut et le bas, Maj (ou une molette latérale) les fait défiler de côté, et la molette au-dessus de la barre de défilement sous les tranches fait défiler de côté.

Beaucoup de dispositions de clavier européennes signalent AltGr comme `Ctrl+Alt` (la valeur par défaut de Désactiver l'aimantation hors macOS) : taper un caractère avec AltGr pendant un glissé désactive l'aimantation. Réassignez Désactiver l'aimantation dans Réglages, Clavier si cela gêne. Dans un champ de texte, AltGr tape toujours son caractère et ne lance jamais de raccourci.

## Champs de texte {#text-fields}

Chaque endroit où l'on tape du texte (noms, recherches, nombres et durées, commentaires) est le même champ. Un clic place le curseur, un glissé sélectionne, un double-clic sélectionne un mot et un triple-clic tout. Gauche et Droite déplacent d'un caractère, avec `Ctrl` (`Opt` sur macOS) d'un mot, `Début` et `Fin` vont aux extrémités de la ligne, et `Maj` avec l'une d'elles sélectionne. `Ctrl+A` sélectionne tout ; `Ctrl+C`, `Ctrl+X` et `Ctrl+V` copient, coupent et collent (`Cmd` sur macOS). `Ctrl+Z` annule dans le champ, `Ctrl+Maj+Z` (et `Ctrl+Y` hors macOS) rétablit ; une suite de lettres est une étape et chaque champ garde les 50 dernières. Tant qu'un champ a le clavier, ces combinaisons lui appartiennent et ne lancent jamais Annuler du morceau ; sans champ, `Ctrl+Z` est Annuler du morceau. Un clic droit sur un champ ouvre son propre menu (Annuler, Rétablir, Couper, Copier, Coller, Supprimer, Tout sélectionner). Les méthodes de saisie composent dans le champ. Entrée et Échap terminent la saisie comme le dit le panneau. Les champs qui prennent des nombres refusent les autres caractères.

## Info-bulles {#tooltips}

Laisser le pointeur sur un bouton environ 0,6 seconde affiche dessous une info-bulle d'une ligne qui dit ce que fait le bouton, avec le raccourci de sa commande quand il en a un (écrit comme le font les menus, si bien qu'une touche que vous réassignez dans Réglages, Clavier s'affiche aussitôt). L'info-bulle se masque à un appui, une touche, la molette, ou quand le pointeur quitte le bouton. Réglages, Workflow, **Afficher les info-bulles** (activé par défaut) les désactive ; la lecture de valeur affichée pendant qu'on fait glisser un bouton ou un fader reste.

## Touches déplacées : touches standard et touches historiques (Windows) {#moved-keys-standard-keys-against-the-legacy-keys-windows}

| Commande | Touches historiques d'Atomik | Touches standard |
|---|---|---|
| Fermer | - | `Ctrl+W` |
| Importer de l'audio | `Ctrl+I` | `Ctrl+Maj+I` |
| Ajouter un marqueur | - | `Maj+M` |
| Aller au repère (droite) | - | `Numpad2`, `Alt+Droite` |
| Aller au repère (gauche) | - | `Numpad1`, `Alt+Gauche` |
| Alt : nouveau calque, couper le dernier | - | `Numpad6` |
| Arrêt | - | `Maj+Entrée`, `Numpad0` |
| Avancer d'une mesure | - | `Numpad8` |
| Avancer d'une mesure | - | `Numpad5` |
| Boucle | `L`, `Maj+L` (rack, sequencer) | `L`, `NumpadDiv` |
| Boucle sur la sélection | - | `Ctrl+L` (sequencer) |
| Boucler la sélection et lire | - | `P` (sequencer) |
| Décompte | - | `Ctrl+P` |
| Déplacer la boucle (bar:+1) | - | `Alt+Maj+Droite` |
| Déplacer la boucle (bar:-1) | - | `Alt+Maj+Gauche` |
| Déplacer la boucle (length:+1) | - | `Alt+Maj+Haut` |
| Déplacer la boucle (length:-1) | - | `Alt+Maj+Bas` |
| Enregistrer | `Ctrl+Espace` (global); `R`, `Maj+R` (rack, sequencer) | `Ctrl+Entrée`, `Ctrl+Espace`, `NumpadMul` |
| Lancer la scène | - | `Maj+Entrée` (launcher) |
| Lancer le clip | - | `Entrée` (launcher) |
| Lecture | - | `NumpadEnter` |
| Lecture / Arrêt | `Espace`, `Maj+Espace` (rack, sequencer) | `Espace` |
| Marqueur précédent | - | `,` |
| Marqueur suivant | - | `.` |
| Métronome | - | `C`, `Numpad9` |
| Overdub : nouveau calque | - | `Numpad3` |
| Reculer d'une mesure | - | `Numpad7` |
| Reculer d'une mesure | - | `Numpad4` |
| Retour à zéro | `Entrée`, `Maj+Entrée` (rack, sequencer) | `NumpadDecimal`, `Entrée` |
| Tempo plus lent | - | `NumpadSub` |
| Tempo plus rapide | - | `NumpadAdd` |
| Toutes les notes coupées | - | `!`, `Maj+!` |
| Aller à la recherche | - | `Ctrl+Maj+F` |
| Changer l'affichage des câbles | - | `K` |
| Chercher un paramètre... | - | `Ctrl+Maj+P` |
| Détacher la table de mixage | - | `Ctrl+Maj+F5` |
| Détacher le rack | - | `Ctrl+Maj+F6` |
| Explorateur de fichiers | `F4` | `F9` |
| Fermer la fenêtre du morceau | - | `Ctrl+Maj+W` |
| Lanceur | - | `F10` |
| Morceau précédent | - | `Ctrl+Maj+Page préc.` |
| Morceau suivant | - | `Ctrl+Maj+Page suiv.` |
| Navigateur d'appareils | `Ctrl+B` | `F3`, `Ctrl+F`, `Ctrl+B` |
| Nouvelle fenêtre de morceau | - | `Ctrl+Maj+N` |
| Ouvrir le morceau dans une nouvelle fenêtre | - | `Ctrl+Maj+O` |
| Palette de commandes... | - | `Ctrl+Maj+K` |
| Rack | - | `F6` |
| Séquenceur seul | - | `F7` |
| Touches de piano | `F9` | `F4` |
| Zoom arrière | `Ctrl+-` | `Ctrl+-`, `Ctrl+Alt+-` (global); `G` (sequencer, editor) |
| Zoom avant | `Ctrl+=`, `Ctrl+Maj+=`, `Ctrl++`, `Ctrl+Maj++` | `Ctrl+=`, `Ctrl++`, `Ctrl+Maj+=`, `Ctrl+Maj++`, `Ctrl+Alt+=`, `Ctrl+Alt++` (global); `H` (sequencer, editor) |
| Éditeur de clip | - | `F8`, `Maj+Tab` (global); `Ctrl+E` (sequencer) |
| Égaliseur à spectre | - | `F2` |
| Coller | - | `Ctrl+V` |
| Coller l'accord | - | `Ctrl+Alt+V` (sequencer) |
| Copier | - | `Ctrl+C` |
| Couper | - | `Ctrl+X` |
| Dupliquer | `Ctrl+D` (rack, sequencer) | `Ctrl+D` |
| Rétablir | `Ctrl+Y`, `Ctrl+Maj+Y`, `Ctrl+Maj+Z` | `Ctrl+Y`, `Ctrl+Maj+Z` |
| Supprimer | `Suppr`, `Retour arrière` (rack, sequencer, editor) | `Suppr`, `Retour arrière` |
| Tout sélectionner | `Ctrl+A` (sequencer, editor) | `Ctrl+A` |
| Sélectionner l'appareil précédent | - | `Haut` (rack) |
| Sélectionner l'appareil suivant | - | `Bas` (rack) |
| Étendre la sélection vers le bas | - | `Maj+Bas` (rack) |
| Étendre la sélection vers le haut | - | `Maj+Haut` (rack) |
| Aimantation activée/désactivée | - | `S` (sequencer, editor) |
| Couper à la tête de lecture | `Ctrl+E` (sequencer) | `Alt+X` (sequencer) |
| Descendre la piste | - | `Ctrl+Alt+Maj+Bas` (sequencer) |
| Fermer le clip | - | `Échap` (sequencer) |
| Fondu enchaîné audio (basculer) | - | `X` (sequencer) |
| Fusionner les voies de notes | - | `Ctrl+R` (sequencer) |
| Grouper les pistes | - | `Ctrl+Maj+T` (sequencer) |
| Hauteur de la piste (ajusté) | - | `Alt+M` (sequencer) |
| Hauteur de la piste (réduire) | - | `Alt+Maj+M` (sequencer) |
| Joindre | - | `Ctrl+J` (sequencer) |
| Jouer la ligne de prise seule | - | `Alt+P` (sequencer) |
| Ligne de prise précédente | - | `Alt+Haut` (sequencer) |
| Ligne de prise suivante | - | `Alt+Bas` (sequencer) |
| Monter la piste | - | `Ctrl+Alt+Maj+Haut` (sequencer) |
| Ouvrir le clip | - | `Entrée` (sequencer) |
| Pistes plus basses | - | `Maj+G` (sequencer) |
| Pistes plus hautes | - | `Maj+H` (sequencer) |
| Réinitialiser la hauteur des pistes | - | `Alt+Z` (sequencer) |
| Suivre la tête de lecture | - | `F` |
| Sélectionner la piste précédente | - | `Haut` (sequencer) |
| Sélectionner la piste suivante | - | `Bas` (sequencer) |
| Sélectionner le clip précédent | - | `Gauche` (sequencer) |
| Sélectionner le clip suivant | - | `Droite` (sequencer) |
| Zoomer sur la sélection | - | `Z`, `Maj+Z` (sequencer); `Z` (editor) |
| Étendre la sélection de pistes vers le bas | - | `Maj+Bas` (sequencer) |
| Étendre la sélection de pistes vers le haut | - | `Maj+Haut` (sequencer) |
| Aimanter à la gamme | - | `Maj+S` (editor) |
| Décaler vers la droite | `Droite` (editor) | `Ctrl+Droite` (editor) |
| Décaler vers la droite d'un temps | `Maj+Droite` (editor) | `Ctrl+Maj+Droite` (editor) |
| Décaler vers la droite d'un tick | `Alt+Droite` (editor) | `Ctrl+Alt+Maj+Droite` (editor) |
| Décaler vers la gauche | `Gauche` (editor) | `Ctrl+Gauche` (editor) |
| Décaler vers la gauche d'un temps | `Maj+Gauche` (editor) | `Ctrl+Maj+Gauche` (editor) |
| Décaler vers la gauche d'un tick | `Alt+Gauche` (editor) | `Ctrl+Alt+Maj+Gauche` (editor) |
| Joindre les notes | - | `Ctrl+J` (editor) |
| Quantifier | `Q`, `Maj+Q` (editor) | `Ctrl+K` (sequencer, editor) |
| Rendre legato (aucun) | - | `Ctrl+Maj+J` (editor) |
| Replier | - | `Maj+F` (editor) |
| Réinitialiser la hauteur des touches | - | `Maj+Z` (editor) |
| Sélectionner la note précédente | - | `Gauche` (editor) |
| Sélectionner la note suivante | - | `Droite` (editor) |
| Touches plus courtes | - | `Maj+G` (editor) |
| Touches plus hautes | - | `Maj+H` (editor) |
| Transformer la sélection en accords | - | `Maj+C` (editor) |
| Transposer d'une octave vers le bas | `Maj+Bas` (editor) | `Ctrl+Maj+Bas` (editor) |
| Transposer d'une octave vers le haut | `Maj+Haut` (editor) | `Ctrl+Maj+Haut` (editor) |
| Transposer vers le bas | `Bas` (editor) | `Ctrl+Bas` (editor) |
| Transposer vers le haut | `Haut` (editor) | `Ctrl+Haut` (editor) |
| Étendre la sélection à la note précédente | - | `Maj+Gauche` (editor) |
| Étendre la sélection à la note suivante | - | `Maj+Droite` (editor) |
| Comparer A/B | - | `Ctrl+Alt+B` |
| Rappeler l'instantané précédent | - | `Ctrl+Alt+Page préc.` |
| Rappeler l'instantané suivant | - | `Ctrl+Alt+Page suiv.` |
| Router vers un nouveau bus de sortie | - | `Ctrl+G` |
| Écouter la référence | - | `Ctrl+Maj+R` |
| Annuler la recherche | - | `Échap` (browser) |
| Créer l'appareil sélectionné | - | `Entrée` (browser) |
| Navigateur : bas | - | `Bas` (browser) |
| Navigateur : haut | - | `Haut` (browser) |
| Créer un canal de mixage | - | `Ctrl+Maj+M` |
| Créer un effet | - | `Ctrl+E` |
| Créer un instrument | - | `Ctrl+I`, `Inser` |
| Créer une piste audio | - | `Ctrl+T` |
| Outil crayon | - | `W` (sequencer, editor) |
| Outil de sélection | - | `Q` (sequencer, editor) |
| Outil enceinte | - | `T` (sequencer, editor) |
| Outil gomme | - | `E` (sequencer, editor) |
| Outil loupe | - | `U` (sequencer) |
| Outil main | - | `I` (sequencer) |
| Outil mute | - | `Y` (sequencer) |
| Outil rasoir | - | `R` (sequencer, editor) |

## Touches standard {#standard-keys}

### Global {#global}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Enregistrer | global | `Ctrl+S` | `Cmd+S` | `Ctrl+S` |
| Enregistrer sous | global | `Ctrl+Maj+S` | `Cmd+Maj+S` | `Ctrl+Maj+S` |
| Fermer | global | `Ctrl+W` | `Cmd+W` | `Ctrl+W` |
| Importer de l'audio | global | `Ctrl+Maj+I` | `Cmd+Opt+I` | `Ctrl+Maj+I` |
| Manuel | global | `Ctrl+F1` | `Cmd+Opt+F1` | `Ctrl+F1` |
| Mode aide | global | `Maj+F1` | `Maj+F1` | `Maj+F1` |
| Nouveau | global | `Ctrl+N` | `Cmd+N` | `Ctrl+N` |
| Ouvrir | global | `Ctrl+O` | `Cmd+O` | `Ctrl+O` |
| Quitter | global | `Ctrl+Q` | `Cmd+Q` | `Ctrl+Q` |
| Raccourcis clavier | global | `F1` | `F1` | `F1` |
| Réglages | global | `Ctrl+,` | `Cmd+,` | `Ctrl+,` |

### Transport {#transport}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Ajouter un marqueur | global | `Maj+M` | `Maj+M` | `Maj+M` |
| Aller au repère (droite) | global | `Numpad2`, `Alt+Droite` | `Numpad2`, `Opt+Droite` | `Numpad2`, `Alt+Droite` |
| Aller au repère (gauche) | global | `Numpad1`, `Alt+Gauche` | `Numpad1`, `Opt+Gauche` | `Numpad1`, `Alt+Gauche` |
| Alt : nouveau calque, couper le dernier | global | `Numpad6` | `Numpad6` | `Numpad6` |
| Arrêt | global | `Maj+Entrée`, `Numpad0` | `Maj+Entrée`, `Numpad0` | `Maj+Entrée`, `Numpad0` |
| Avancer d'une mesure | global | `Numpad8` | `Numpad8` | `Numpad8` |
| Avancer d'une mesure | global | `Numpad5` | `Numpad5` | `Numpad5` |
| Boucle | global | `L`, `NumpadDiv` | `L`, `NumpadDiv` | `L`, `NumpadDiv` |
| Boucle sur la sélection | sequencer | `Ctrl+L` | `Cmd+L` | `Ctrl+L` |
| Boucler la sélection et lire | sequencer | `P` | `P` | `P` |
| Décompte | global | `Ctrl+P` | `Cmd+P` | `Ctrl+P` |
| Déplacer la boucle (bar:+1) | global | `Alt+Maj+Droite` | `Opt+Maj+Droite` | `Alt+Maj+Droite` |
| Déplacer la boucle (bar:-1) | global | `Alt+Maj+Gauche` | `Opt+Maj+Gauche` | `Alt+Maj+Gauche` |
| Déplacer la boucle (length:+1) | global | `Alt+Maj+Haut` | `Opt+Maj+Haut` | `Alt+Maj+Haut` |
| Déplacer la boucle (length:-1) | global | `Alt+Maj+Bas` | `Opt+Maj+Bas` | `Alt+Maj+Bas` |
| Enregistrer | global | `Ctrl+Entrée`, `Ctrl+Espace`, `NumpadMul` | `Cmd+Entrée`, `Cmd+Espace`, `NumpadMul` | `Ctrl+Entrée`, `Ctrl+Espace`, `NumpadMul` |
| Lancer la scène | launcher | `Maj+Entrée` | `Maj+Entrée` | `Maj+Entrée` |
| Lancer le clip | launcher | `Entrée` | `Entrée` | `Entrée` |
| Lecture | global | `NumpadEnter` | `NumpadEnter` | `NumpadEnter` |
| Lecture / Arrêt | global | `Espace` | `Espace` | `Espace` |
| Marqueur précédent | global | `,` | `,` | `,` |
| Marqueur suivant | global | `.` | `.` | `.` |
| Métronome | global | `C`, `Numpad9` | `C`, `Numpad9` | `C`, `Numpad9` |
| Overdub : nouveau calque | global | `Numpad3` | `Numpad3` | `Numpad3` |
| Reculer d'une mesure | global | `Numpad7` | `Numpad7` | `Numpad7` |
| Reculer d'une mesure | global | `Numpad4` | `Numpad4` | `Numpad4` |
| Retour à zéro | global | `NumpadDecimal`, `Entrée` | `NumpadDecimal`, `Entrée` | `NumpadDecimal`, `Entrée` |
| Tempo plus lent | global | `NumpadSub` | `NumpadSub` | `NumpadSub` |
| Tempo plus rapide | global | `NumpadAdd` | `NumpadAdd` | `NumpadAdd` |
| Toutes les notes coupées | global | `!`, `Maj+!` | `!`, `Maj+!` | `!`, `Maj+!` |

### Affichage {#view}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Agrandir la console | global | `Maj+F5` | `Maj+F5` | `Maj+F5` |
| Aller à la recherche | global | `Ctrl+Maj+F` | `Cmd+Maj+F` | `Ctrl+Maj+F` |
| Arrière du rack | global | `Tab` | `Tab` | `Tab` |
| Changer l'affichage des câbles | global | `K` | `K` | `K` |
| Chercher un paramètre... | global | `Ctrl+Maj+P` | `Cmd+Maj+P` | `Ctrl+Maj+P` |
| Console | global | `F5` | `F5`, `Cmd+Opt+Digit5` | `F5` |
| Détacher la table de mixage | global | `Ctrl+Maj+F5` | `Cmd+Maj+F5` | `Ctrl+Maj+F5` |
| Détacher le rack | global | `Ctrl+Maj+F6` | `Cmd+Maj+F6` | `Ctrl+Maj+F6` |
| Explorateur de fichiers | global | `F9` | `F9`, `Cmd+Opt+Digit9` | `F9` |
| Fermer la fenêtre du morceau | global | `Ctrl+Maj+W` | `Cmd+Maj+W` | `Ctrl+Maj+W` |
| Lanceur | global | `F10` | `F10` | `F10` |
| Morceau précédent | global | `Ctrl+Maj+Page préc.` | `Cmd+Maj+Page préc.` | `Ctrl+Maj+Page préc.` |
| Morceau suivant | global | `Ctrl+Maj+Page suiv.` | `Cmd+Maj+Page suiv.` | `Ctrl+Maj+Page suiv.` |
| Navigateur d'appareils | global | `F3`, `Ctrl+F`, `Ctrl+B` | `F3`, `Cmd+F`, `Cmd+B`, `Cmd+Opt+Digit3` | `F3`, `Ctrl+F`, `Ctrl+B` |
| Nouvelle fenêtre de morceau | global | `Ctrl+Maj+N` | `Cmd+Maj+N` | `Ctrl+Maj+N` |
| Ouvrir le morceau dans une nouvelle fenêtre | global | `Ctrl+Maj+O` | `Cmd+Maj+O` | `Ctrl+Maj+O` |
| Palette de commandes... | global | `Ctrl+Maj+K` | `Cmd+Maj+K` | `Ctrl+Maj+K` |
| Rack | global | `F6` | `F6`, `Cmd+Opt+Digit6` | `F6` |
| Séquenceur seul | global | `F7` | `F7`, `Cmd+Opt+Digit7` | `F7` |
| Taille réelle | global | `Ctrl+0` | `Cmd+0` | `Ctrl+0` |
| Touches de piano | global | `F4` | `F4`, `Cmd+Opt+Digit4` | `F4` |
| Zoom arrière | global | `Ctrl+-`, `Ctrl+Alt+-` | `Cmd+-`, `Cmd+Opt+-` | `Ctrl+-`, `Ctrl+Alt+-` |
| Zoom arrière | sequencer | `G` | `G` | `G` |
| Zoom arrière | editor | `G` | `G` | `G` |
| Zoom avant | global | `Ctrl+=`, `Ctrl++`, `Ctrl+Maj+=`, `Ctrl+Maj++`, `Ctrl+Alt+=`, `Ctrl+Alt++` | `Cmd+=`, `Cmd++`, `Cmd+Maj+=`, `Cmd+Maj++`, `Cmd+Opt+=`, `Cmd+Opt++` | `Ctrl+=`, `Ctrl++`, `Ctrl+Maj+=`, `Ctrl+Maj++`, `Ctrl+Alt+=`, `Ctrl+Alt++` |
| Zoom avant | sequencer | `H` | `H` | `H` |
| Zoom avant | editor | `H` | `H` | `H` |
| Éditeur de clip | global | `F8`, `Maj+Tab` | `F8`, `Maj+Tab`, `Cmd+Opt+Digit8` | `F8`, `Maj+Tab` |
| Éditeur de clip | sequencer | `Ctrl+E` | `Cmd+E` | `Ctrl+E` |
| Égaliseur à spectre | global | `F2` | `F2`, `Cmd+Opt+Digit2` | `F2` |

### Édition {#edit}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Annuler | global | `Ctrl+Z` | `Cmd+Z` | `Ctrl+Z` |
| Coller | global | `Ctrl+V` | `Cmd+V` | `Ctrl+V` |
| Coller l'accord | sequencer | `Ctrl+Alt+V` | `Cmd+Opt+V` | `Ctrl+Alt+V` |
| Copier | global | `Ctrl+C` | `Cmd+C` | `Ctrl+C` |
| Couper | global | `Ctrl+X` | `Cmd+X` | `Ctrl+X` |
| Dupliquer | global | `Ctrl+D` | `Cmd+D` | `Ctrl+D` |
| Rétablir | global | `Ctrl+Y`, `Ctrl+Maj+Z` | `Cmd+Y`, `Cmd+Maj+Z` | `Ctrl+Y`, `Ctrl+Maj+Z` |
| Supprimer | global | `Suppr`, `Retour arrière` | `Suppr`, `Retour arrière` | `Suppr`, `Retour arrière` |
| Tout sélectionner | global | `Ctrl+A` | `Cmd+A` | `Ctrl+A` |

### Rack {#rack}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Combiner | rack | `Ctrl+Maj+G` | `Cmd+Maj+G` | `Ctrl+Maj+G` |
| Parcourir les patchs | rack | `Ctrl+Maj+B` | `Cmd+Maj+B` | `Ctrl+Maj+B` |
| Patch précédent | rack | `Ctrl+Gauche` | `Cmd+Gauche` | `Ctrl+Gauche` |
| Patch suivant | rack | `Ctrl+Droite` | `Cmd+Droite` | `Ctrl+Droite` |
| Sélectionner l'appareil précédent | rack | `Haut` | `Haut` | `Haut` |
| Sélectionner l'appareil suivant | rack | `Bas` | `Bas` | `Bas` |
| Séparer | rack | `Ctrl+Maj+U` | `Cmd+Maj+U` | `Ctrl+Maj+U` |
| Étendre la sélection vers le bas | rack | `Maj+Bas` | `Maj+Bas` | `Maj+Bas` |
| Étendre la sélection vers le haut | rack | `Maj+Haut` | `Maj+Haut` | `Maj+Haut` |

### Séquenceur {#sequencer}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Aimantation activée/désactivée | sequencer | `S` | `S` | `S` |
| Aimantation activée/désactivée | editor | `S` | `S` | `S` |
| Couper les clips | sequencer | `M` | `M` | `M` |
| Couper à la tête de lecture | sequencer | `Alt+X` | `Opt+X` | `Alt+X` |
| Descendre la piste | sequencer | `Ctrl+Alt+Maj+Bas` | `Cmd+Opt+Maj+Bas` | `Ctrl+Alt+Maj+Bas` |
| Décaler les clips vers la droite | sequencer | `Ctrl+Droite` | `Cmd+Droite` | `Ctrl+Droite` |
| Décaler les clips vers la droite d'un temps | sequencer | `Ctrl+Maj+Droite` | `Cmd+Maj+Droite` | `Ctrl+Maj+Droite` |
| Décaler les clips vers la droite d'un tick | sequencer | `Ctrl+Alt+Maj+Droite` | `Cmd+Opt+Maj+Droite` | `Ctrl+Alt+Maj+Droite` |
| Décaler les clips vers la gauche | sequencer | `Ctrl+Gauche` | `Cmd+Gauche` | `Ctrl+Gauche` |
| Décaler les clips vers la gauche d'un temps | sequencer | `Ctrl+Maj+Gauche` | `Cmd+Maj+Gauche` | `Ctrl+Maj+Gauche` |
| Décaler les clips vers la gauche d'un tick | sequencer | `Ctrl+Alt+Maj+Gauche` | `Cmd+Opt+Maj+Gauche` | `Ctrl+Alt+Maj+Gauche` |
| Fermer le clip | sequencer | `Échap` | `Échap` | `Échap` |
| Fondu enchaîné audio (basculer) | sequencer | `X` | `X` | `X` |
| Fusionner les voies de notes | sequencer | `Ctrl+R` | `Cmd+R` | `Ctrl+R` |
| Grouper les pistes | sequencer | `Ctrl+Maj+T` | `Cmd+Maj+T` | `Ctrl+Maj+T` |
| Hauteur de la piste (ajusté) | sequencer | `Alt+M` | `Opt+M` | `Alt+M` |
| Hauteur de la piste (réduire) | sequencer | `Alt+Maj+M` | `Opt+Maj+M` | `Alt+Maj+M` |
| Joindre | sequencer | `Ctrl+J` | `Cmd+J` | `Ctrl+J` |
| Jouer la ligne de prise seule | sequencer | `Alt+P` | `Opt+P` | `Alt+P` |
| Ligne de prise précédente | sequencer | `Alt+Haut` | `Opt+Haut` | `Alt+Haut` |
| Ligne de prise suivante | sequencer | `Alt+Bas` | `Opt+Bas` | `Alt+Bas` |
| Monter la piste | sequencer | `Ctrl+Alt+Maj+Haut` | `Cmd+Opt+Maj+Haut` | `Ctrl+Alt+Maj+Haut` |
| Ouvrir le clip | sequencer | `Entrée` | `Entrée` | `Entrée` |
| Pistes plus basses | sequencer | `Maj+G` | `Maj+G` | `Maj+G` |
| Pistes plus hautes | sequencer | `Maj+H` | `Maj+H` | `Maj+H` |
| Réinitialiser la hauteur des pistes | sequencer | `Alt+Z` | `Opt+Z` | `Alt+Z` |
| Suivre la tête de lecture | global | `F` | `F` | `F` |
| Sélectionner la piste précédente | sequencer | `Haut` | `Haut` | `Haut` |
| Sélectionner la piste suivante | sequencer | `Bas` | `Bas` | `Bas` |
| Sélectionner le clip précédent | sequencer | `Gauche` | `Gauche` | `Gauche` |
| Sélectionner le clip suivant | sequencer | `Droite` | `Droite` | `Droite` |
| Zoomer sur la sélection | sequencer | `Z`, `Maj+Z` | `Z`, `Maj+Z` | `Z`, `Maj+Z` |
| Zoomer sur la sélection | editor | `Z` | `Z` | `Z` |
| Étendre la sélection de pistes vers le bas | sequencer | `Maj+Bas` | `Maj+Bas` | `Maj+Bas` |
| Étendre la sélection de pistes vers le haut | sequencer | `Maj+Haut` | `Maj+Haut` | `Maj+Haut` |

### Éditeur {#editor}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Afficher les voies de contrôleurs | editor | `Maj+L` | `Maj+L` | `Maj+L` |
| Aimanter à la gamme | editor | `Maj+S` | `Maj+S` | `Maj+S` |
| Ajouter un marqueur au pointeur | editor | `I` | `I` | `I` |
| Couper les notes | editor | `M` | `M` | `M` |
| Décaler vers la droite | editor | `Ctrl+Droite` | `Cmd+Droite` | `Ctrl+Droite` |
| Décaler vers la droite d'un temps | editor | `Ctrl+Maj+Droite` | `Cmd+Maj+Droite` | `Ctrl+Maj+Droite` |
| Décaler vers la droite d'un tick | editor | `Ctrl+Alt+Maj+Droite` | `Cmd+Opt+Maj+Droite` | `Ctrl+Alt+Maj+Droite` |
| Décaler vers la gauche | editor | `Ctrl+Gauche` | `Cmd+Gauche` | `Ctrl+Gauche` |
| Décaler vers la gauche d'un temps | editor | `Ctrl+Maj+Gauche` | `Cmd+Maj+Gauche` | `Ctrl+Maj+Gauche` |
| Décaler vers la gauche d'un tick | editor | `Ctrl+Alt+Maj+Gauche` | `Cmd+Opt+Maj+Gauche` | `Ctrl+Alt+Maj+Gauche` |
| Joindre les notes | editor | `Ctrl+J` | `Cmd+J` | `Ctrl+J` |
| Quantifier | sequencer | `Ctrl+K` | `Cmd+K` | `Ctrl+K` |
| Quantifier | editor | `Ctrl+K` | `Cmd+K` | `Ctrl+K` |
| Rendre legato (aucun) | editor | `Ctrl+Maj+J` | `Cmd+Maj+J` | `Ctrl+Maj+J` |
| Replier | editor | `Maj+F` | `Maj+F` | `Maj+F` |
| Réinitialiser la hauteur des touches | editor | `Maj+Z` | `Maj+Z` | `Maj+Z` |
| Sélectionner la note précédente | editor | `Gauche` | `Gauche` | `Gauche` |
| Sélectionner la note suivante | editor | `Droite` | `Droite` | `Droite` |
| Tablature : corde plus aiguë | tab | `Alt+Haut` | `Opt+Haut` | `Alt+Haut` |
| Tablature : corde plus grave | tab | `Alt+Bas` | `Opt+Bas` | `Alt+Bas` |
| Tablature : curseur vers le bas | tab | `Bas` | `Bas` | `Bas` |
| Tablature : curseur vers le haut | tab | `Haut` | `Haut` | `Haut` |
| Tablature : curseur à droite | tab | `Droite` | `Droite` | `Droite` |
| Tablature : curseur à gauche | tab | `Gauche` | `Gauche` | `Gauche` |
| Tablature : mesure précédente | tab | `Ctrl+Gauche` | `Cmd+Gauche` | `Ctrl+Gauche` |
| Tablature : mesure suivante | tab | `Ctrl+Droite` | `Cmd+Droite` | `Ctrl+Droite` |
| Tablature : note en triolet | tab | `Y` | `Y` | `Y` |
| Tablature : note pointée | tab | `.` | `.` | `.` |
| Tablature : technique (accent) | tab | `A` | `A` | `A` |
| Tablature : technique (effacer) | tab | `Maj+C` | `Maj+C` | `Maj+C` |
| Tablature : technique (glissé décalé) | tab | `Maj+S` | `Maj+S` | `Maj+S` |
| Tablature : technique (glissé legato) | tab | `Maj+L` | `Maj+L` | `Maj+L` |
| Tablature : technique (hammer-on) | tab | `Maj+H` | `Maj+H` | `Maj+H` |
| Tablature : technique (harmonique artificielle) | tab | `Maj+N` | `Maj+N` | `Maj+N` |
| Tablature : technique (harmonique) | tab | `N` | `N` | `N` |
| Tablature : technique (laisser sonner) | tab | `R` | `R` | `R` |
| Tablature : technique (note fantôme) | tab | `G` | `G` | `G` |
| Tablature : technique (note morte) | tab | `D` | `D` | `D` |
| Tablature : technique (note tapée) | tab | `Maj+T` | `Maj+T` | `Maj+T` |
| Tablature : technique (pré-tiré) | tab | `Alt+B` | `Opt+B` | `Alt+B` |
| Tablature : technique (pull-off) | tab | `Maj+P` | `Maj+P` | `Maj+P` |
| Tablature : technique (tiré puis relâché) | tab | `Maj+B` | `Maj+B` | `Maj+B` |
| Tablature : technique (tiré) | tab | `B` | `B` | `B` |
| Tablature : technique (vibrato large) | tab | `Maj+V` | `Maj+V` | `Maj+V` |
| Tablature : technique (vibrato) | tab | `V` | `V` | `V` |
| Tablature : technique (étouffé (paume)) | tab | `M` | `M` | `M` |
| Tablature : transposer (bas) | tab | `Ctrl+Bas` | `Cmd+Bas` | `Ctrl+Bas` |
| Tablature : transposer (haut) | tab | `Ctrl+Haut` | `Cmd+Haut` | `Ctrl+Haut` |
| Tablature : transposer (octave inférieure) | tab | `Ctrl+Maj+Bas` | `Cmd+Maj+Bas` | `Ctrl+Maj+Bas` |
| Tablature : transposer (octave supérieure) | tab | `Ctrl+Maj+Haut` | `Cmd+Maj+Haut` | `Ctrl+Maj+Haut` |
| Tablature : valeur de note (blanche) | tab | `H` | `H` | `H` |
| Tablature : valeur de note (croche) | tab | `E` | `E` | `E` |
| Tablature : valeur de note (double croche) | tab | `S` | `S` | `S` |
| Tablature : valeur de note (noire) | tab | `Q` | `Q` | `Q` |
| Tablature : valeur de note (quadruple croche) | tab | `X` | `X` | `X` |
| Tablature : valeur de note (ronde) | tab | `W` | `W` | `W` |
| Tablature : valeur de note (triple croche) | tab | `T` | `T` | `T` |
| Tablature : zoom arrière | tab | `-` | `-` | `-` |
| Tablature : zoom avant | tab | `=` | `=` | `=` |
| Tablature : étendre la sélection (bas) | tab | `Maj+Bas` | `Maj+Bas` | `Maj+Bas` |
| Tablature : étendre la sélection (droite) | tab | `Maj+Droite` | `Maj+Droite` | `Maj+Droite` |
| Tablature : étendre la sélection (gauche) | tab | `Maj+Gauche` | `Maj+Gauche` | `Maj+Gauche` |
| Tablature : étendre la sélection (haut) | tab | `Maj+Haut` | `Maj+Haut` | `Maj+Haut` |
| Touches plus courtes | editor | `Maj+G` | `Maj+G` | `Maj+G` |
| Touches plus hautes | editor | `Maj+H` | `Maj+H` | `Maj+H` |
| Transformer la sélection en accords | editor | `Maj+C` | `Maj+C` | `Maj+C` |
| Transposer d'une octave vers le bas | editor | `Ctrl+Maj+Bas` | `Cmd+Maj+Bas` | `Ctrl+Maj+Bas` |
| Transposer d'une octave vers le haut | editor | `Ctrl+Maj+Haut` | `Cmd+Maj+Haut` | `Ctrl+Maj+Haut` |
| Transposer vers le bas | editor | `Ctrl+Bas` | `Cmd+Bas` | `Ctrl+Bas` |
| Transposer vers le haut | editor | `Ctrl+Haut` | `Cmd+Haut` | `Ctrl+Haut` |
| Éditeur audio : découpe | editor | `2` | `2` | `2` |
| Éditeur audio : voix | editor | `3` | `3` | `3` |
| Éditeur audio : vue d'ensemble | editor | `1` | `1` | `1` |
| Étendre la sélection à la note précédente | editor | `Maj+Gauche` | `Maj+Gauche` | `Maj+Gauche` |
| Étendre la sélection à la note suivante | editor | `Maj+Droite` | `Maj+Droite` | `Maj+Droite` |

### Table de mixage {#mixer}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Aller au dernier canal | mixer | `Fin`, `Maj+Fin` | `Fin`, `Maj+Fin` | `Fin`, `Maj+Fin` |
| Aller au premier canal | mixer | `Début`, `Maj+Début` | `Début`, `Maj+Début` | `Début`, `Maj+Début` |
| Augmenter la commande active | mixer | `Haut` | `Haut` | `Haut` |
| Augmenter la commande active (fin) | mixer | `Maj+Haut` | `Maj+Haut` | `Maj+Haut` |
| Augmenter la commande active de dix | mixer | `Page préc.` | `Page préc.` | `Page préc.` |
| Augmenter la commande active de dix (fin) | mixer | `Maj+Page préc.` | `Maj+Page préc.` | `Maj+Page préc.` |
| Baisser la commande active | mixer | `Bas` | `Bas` | `Bas` |
| Baisser la commande active (fin) | mixer | `Maj+Bas` | `Maj+Bas` | `Maj+Bas` |
| Baisser la commande active de dix | mixer | `Page suiv.` | `Page suiv.` | `Page suiv.` |
| Baisser la commande active de dix (fin) | mixer | `Maj+Page suiv.` | `Maj+Page suiv.` | `Maj+Page suiv.` |
| Comparer A/B | global | `Ctrl+Alt+B` | `Cmd+Opt+B` | `Ctrl+Alt+B` |
| Défiler les canaux vers la droite | mixer | `Ctrl+Droite`, `Ctrl+Maj+Droite` | `Cmd+Droite`, `Cmd+Maj+Droite` | `Ctrl+Droite`, `Ctrl+Maj+Droite` |
| Défiler les canaux vers la gauche | mixer | `Ctrl+Gauche`, `Ctrl+Maj+Gauche` | `Cmd+Gauche`, `Cmd+Maj+Gauche` | `Ctrl+Gauche`, `Ctrl+Maj+Gauche` |
| Rappeler l'instantané précédent | global | `Ctrl+Alt+Page préc.` | `Cmd+Opt+Page préc.` | `Ctrl+Alt+Page préc.` |
| Rappeler l'instantané suivant | global | `Ctrl+Alt+Page suiv.` | `Cmd+Opt+Page suiv.` | `Ctrl+Alt+Page suiv.` |
| Router vers un nouveau bus de sortie | global | `Ctrl+G` | `Cmd+G` | `Ctrl+G` |
| Sélectionner le canal précédent | mixer | `Gauche` | `Gauche` | `Gauche` |
| Sélectionner le canal suivant | mixer | `Droite` | `Droite` | `Droite` |
| Écouter la référence | global | `Ctrl+Maj+R` | `Cmd+Maj+R` | `Ctrl+Maj+R` |
| Étendre la sélection vers la droite | mixer | `Maj+Droite` | `Maj+Droite` | `Maj+Droite` |
| Étendre la sélection vers la gauche | mixer | `Maj+Gauche` | `Maj+Gauche` | `Maj+Gauche` |

### Navigateur {#browser}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Annuler la recherche | browser | `Échap` | `Échap` | `Échap` |
| Créer l'appareil sélectionné | browser | `Entrée` | `Entrée` | `Entrée` |
| Navigateur : bas | browser | `Bas` | `Bas` | `Bas` |
| Navigateur : haut | browser | `Haut` | `Haut` | `Haut` |

### Fichiers {#files}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Arrêter l'aperçu | files | `Échap` | `Échap` | `Échap` |
| Basculer le favori | files | `F` | `F` | `F` |
| Chercher des fichiers | files | `Ctrl+F` | `Cmd+F` | `Ctrl+F` |
| Dernier élément | files | `Fin` | `Fin` | `Fin` |
| Dossier parent | files | `Retour arrière`, `Alt+Haut` | `Retour arrière`, `Opt+Haut` | `Retour arrière`, `Alt+Haut` |
| Déplier | files | `Droite` | `Droite` | `Droite` |
| Lecture / arrêt de l'aperçu | files | `Espace` | `Espace` | `Espace` |
| Ouvrir | files | `Entrée` | `Entrée` | `Entrée` |
| Page précédente | files | `Page préc.` | `Page préc.` | `Page préc.` |
| Page suivante | files | `Page suiv.` | `Page suiv.` | `Page suiv.` |
| Premier élément | files | `Début` | `Début` | `Début` |
| Renommer l'élément de la bibliothèque | files | `F2` | `F2` | `F2` |
| Replier | files | `Gauche` | `Gauche` | `Gauche` |
| Supprimer l'élément de la bibliothèque | files | `Suppr` | `Suppr` | `Suppr` |
| Sélectionner tous les fichiers | files | `Ctrl+A` | `Cmd+A` | `Ctrl+A` |
| Élément précédent | files | `Haut` | `Haut` | `Haut` |
| Élément suivant | files | `Bas` | `Bas` | `Bas` |

### Touches de piano {#piano-keys}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Jouer une note (do (haut)) | keys | `K` | `K` | `K` |
| Jouer une note (do dièse (haut)) | keys | `O` | `O` | `O` |
| Jouer une note (do dièse) | keys | `W` | `W` | `W` |
| Jouer une note (do) | keys | `A` | `A` | `A` |
| Jouer une note (fa (haut)) | keys | `'`, `"` | `'`, `"` | `'`, `"` |
| Jouer une note (fa dièse) | keys | `T` | `T` | `T` |
| Jouer une note (fa) | keys | `F` | `F` | `F` |
| Jouer une note (la dièse) | keys | `U` | `U` | `U` |
| Jouer une note (la) | keys | `H` | `H` | `H` |
| Jouer une note (mi (haut)) | keys | `;`, `:` | `;`, `:` | `;`, `:` |
| Jouer une note (mi) | keys | `D` | `D` | `D` |
| Jouer une note (ré (haut)) | keys | `L` | `L` | `L` |
| Jouer une note (ré dièse (haut)) | keys | `P` | `P` | `P` |
| Jouer une note (ré dièse) | keys | `E` | `E` | `E` |
| Jouer une note (ré) | keys | `S` | `S` | `S` |
| Jouer une note (si) | keys | `J` | `J` | `J` |
| Jouer une note (sol dièse) | keys | `Y` | `Y` | `Y` |
| Jouer une note (sol) | keys | `G` | `G` | `G` |
| Octave inférieure | keys | `Z` | `Z` | `Z` |
| Octave supérieure | keys | `X` | `X` | `X` |
| Régler la vélocité (1) | keys | `1` | `1` | `1` |
| Régler la vélocité (10) | keys | `0` | `0` | `0` |
| Régler la vélocité (2) | keys | `2` | `2` | `2` |
| Régler la vélocité (3) | keys | `3` | `3` | `3` |
| Régler la vélocité (4) | keys | `4` | `4` | `4` |
| Régler la vélocité (5) | keys | `5` | `5` | `5` |
| Régler la vélocité (6) | keys | `6` | `6` | `6` |
| Régler la vélocité (7) | keys | `7` | `7` | `7` |
| Régler la vélocité (8) | keys | `8` | `8` | `8` |
| Régler la vélocité (9) | keys | `9` | `9` | `9` |

### Créer {#create}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Créer un canal de mixage | global | `Ctrl+Maj+M` | `Cmd+Maj+M` | `Ctrl+Maj+M` |
| Créer un effet | global | `Ctrl+E` | `Cmd+E` | `Ctrl+E` |
| Créer un instrument | global | `Ctrl+I`, `Inser` | `Cmd+I` | `Ctrl+I`, `Inser` |
| Créer une piste audio | global | `Ctrl+T` | `Cmd+T` | `Ctrl+T` |

### Outils {#tools}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Outil crayon | sequencer | `W` | `W` | `W` |
| Outil crayon | editor | `W` | `W` | `W` |
| Outil de sélection | sequencer | `Q` | `Q` | `Q` |
| Outil de sélection | editor | `Q` | `Q` | `Q` |
| Outil enceinte | sequencer | `T` | `T` | `T` |
| Outil enceinte | editor | `T` | `T` | `T` |
| Outil gomme | sequencer | `E` | `E` | `E` |
| Outil gomme | editor | `E` | `E` | `E` |
| Outil loupe | sequencer | `U` | `U` | `U` |
| Outil main | sequencer | `I` | `I` | `I` |
| Outil mute | sequencer | `Y` | `Y` | `Y` |
| Outil rasoir | sequencer | `R` | `R` | `R` |
| Outil rasoir | editor | `R` | `R` | `R` |

Commandes sans touche dans ce profil (les menus et les Réglages y mènent toujours) : Canaux d'entrée, Canaux de sortie, Plugins, Réanalyser les ports MIDI, Redémarrer l'audio, Séparer les stems, Effacer l'automation, Éditer l'automation, Régler la valeur d'automation fixe, Calibrer la rotation de la caméra, Aperçu de la caméra, Caméra, Boutons du contrôleur : page suivante, Boutons du contrôleur : page précédente, Créer un appareil, Créer un player, Copier les réglages, Historique d'annulation, Coller les réglages, Options de quantification, Basculer entre piano roll et tablature, Appliquer la courbe aux notes sélectionnées, Arpéger la sélection, Ajouter des marqueurs aux transitoires, Rétablir les marqueurs, Couper le clip aux marqueurs, Clip vers le découpeur de boucles, Écouter les notes, Découper les notes, Caler la sélection sur la gamme, Détecter la gamme d'après les notes, Éditer l'expression des notes, Extraire les notes vers une nouvelle voie, Ajuster les notes aux accords, Régler le repli, Suivre la tonalité du morceau, Outil de notes, Sélectionner les notes de même hauteur, Régler la durée des notes de l'arpège, Régler les octaves de l'arpège, Régler le motif de l'arpège, Choisir l'accord, Choisir le renversement, Choisir la gamme, Choisir la fondamentale, Régler l'amplitude du strum, Régler le sens du strum, Appliquer la transposition à la grille, Grille : défilement automatique, Grille : capodastre, Supprimer la grille d'accords, Modifier la grille d'accords en texte, Grille : taille du texte, Nouvelle grille d'accords, Renommer la grille d'accords, Choisir la grille d'accords, Grille : dièses ou bémols, Grille : transposer l'affichage, Modifier les pas, Inverser les pas, Pas aléatoires, Décaler les pas, Strummer la sélection, Tablature : amplitude du tiré, Tablature : changer le doigté, Tablature : instrument, accordage, capo, Alléger les points de contrôleur, Entraîneur d'accords : temps par accord, Entraîneur d'accords : tempo, Entraîneur d'accords : capodastre, Entraîneur d'accords : saisir une liste d'accords, Entraîneur d'accords : instrument, Entraîneur d'accords : tonalité, Entraîneur d'accords : mode d'exercice, Entraîneur d'accords : ordre des accords, Entraîneur d'accords : accord de la paire, Entraîneur d'accords : réinitialiser les scores, Entraîneur d'accords : septièmes, Entraîneur d'accords : passer l'accord, Entraîneur d'accords : origine des accords, Entraîneur d'accords : démarrer ou arrêter, Entraîneur d'accords : accordage suivant, Entraîneur d'accords : doigté, Corriger la hauteur des notes, Aplatir la dérive des notes, Réinitialiser les retouches de hauteur, Corriger selon la tonalité du morceau, Annuler l'archive du projet, Annuler la collecte ou le déplacement, Vider les morceaux récents, Rassembler les fichiers, Aperçu de la collecte des fichiers, Convertir la fréquence d'échantillonnage, Réinitialiser le morceau par défaut, Utiliser le morceau comme morceau par défaut, Cibles de livraison, Exporter l'archive du projet, Exporter la liste des paramètres de l'appareil, Exporter le film avec le mix, Exporter pour un usage, Exporter la boucle en fichier audio, Exporter le clip MIDI, Exporter le morceau en MIDI, Exporter la piste MIDI, Exporter le patch, Exporter la sélection en fichier audio, Exporter la grille d'accords en PDF, Exporter la grille d'accords en texte, Exporter le morceau en fichier audio, Exporter les stems en fichiers audio, Exporter la tablature en MusicXML, Exporter la tablature en PDF, Exporter la tablature en texte, Importer une archive de projet, Importer du MIDI, Importer le MIDI comme morceau, Importer une grille d'accords, Importer un fichier de tablature ou de partition, Importer la tablature ou la partition comme morceau, Détecter les accords à l'import MIDI, Résolution de l'export MIDI, Déplacer le projet, Nouveau à la fréquence, Nouveau depuis un modèle, Ouvrir un récent, Relier l'audio manquant, Restaurer une sauvegarde, Enregistrer comme modèle, Enregistrer une version, Partager le projet, Afficher le dossier des modèles, Informations du morceau, Analyste du son, Écran de démarrage, Rack de stress, Morceau de stress, Versions, À propos d'Atomik Rack, Rechercher des mises à jour, Documentation, Exporter le diagnostic, Visite guidée, Signaler un problème, Retour à l'arrangement, Clip vers le lanceur, Nouvelle scène, Quantification du lancement, Enregistrer les lancements, Arrêter tous les clips, La scène arrête les pistes vides, Clip du lanceur vers l'arrangement, Assigner à une macro, Assigner à une nouvelle banque de macros, Nouvelle banque de macros, Macros..., Retirer de la macro, Effacer les assignations MIDI, Exporter un préréglage d'assignations MIDI..., Oublier le MIDI, Importer un préréglage d'assignations MIDI..., Apprentissage MIDI, Apprentissage MIDI, Verrouiller les surfaces sur la sélection, Assigner une commande, Assignations MIDI..., Ajouter un canal, Ajouter une piste de référence..., Ajouter à un VCA, La basse sera-t-elle audible ?..., Contourner tous les inserts, Vérifier le mix..., Vider les inserts, Copier les réglages du canal, Copier les inserts du canal, Copier les niveaux du canal, Copier le mute et le solo du canal, Copier les envois du canal, Créer un canal parallèle, Créer un VCA depuis la sélection, Créer un VCA depuis le dossier, Supprimer l'instantané, Supprimer le VCA, Terminer la comparaison, Contrôle kick et basse, Sub de club, Casque, Mono, Petite enceinte, Lier les faders, Verrouiller / déverrouiller le canal, Carte de masquage..., Mode du vumètre, Morpher vers l'instantané précédent, Morpher vers l'instantané suivant, Durée du morphing, Piste de référence suivante, Coller les réglages du canal, Rappeler l'instantané, Bande de référence, Début de la référence, Caler sur le niveau de référence, Panneau de référence..., Solo stéréo de la référence, Supprimer le canal, Retirer du VCA, Retirer la piste de référence, Réinitialiser le canal, Router le dossier vers un nouveau bus, Router le groupe vers un nouveau bus, Sélectionner la piste de référence, Afficher le phasemètre, Afficher le spectre, L'instantané inclut les effets, Mémoriser l'instantané, Compensation de latence, Inverser la polarité, Aligner les retours, Les envois suivent le bus, Arrêter le routage du dossier, Arrêter le routage du groupe, Mettre à jour l'instantané, Configuration audio et MIDI, Gestionnaire de plugins, Apprendre l'automation depuis la fenêtre du plugin, Apprendre le MIDI depuis la fenêtre du plugin, Passer le chargement du plugin, Pratique : décompte, Pratique : écouter et noter, Pratique : calibrer la latence d'écoute, Pratique : effacer les résultats, Pratique : entrée d'écoute suivante, Pratique : décalage d'écoute, Pratique : précision d'un passage propre, Pratique : boucler la sélection, Pratique : vitesse, Pratique : plus lent, Pratique : plus rapide, Pratique : entraîneur de vitesse, Pratique : passages par palier, Pratique : vitesse de départ, Pratique : palier de l'entraîneur, Pratique : vitesse visée, Ajouter une colonne, Ajouter un effet d'insert..., Ajouter un effet d'envoi..., Utiliser comme effet d'envoi, Routage auto, Test A/B à l'aveugle..., Nettoyer la basse, Déconnecter, Aller à la piste, Importer les données du casque, Choisir le fichier de données du casque, Retirer les données du casque, Brancher la prise, Charger un patch de chaîne master..., Choisir le port de sortie MIDI, Choisir le port de sortie MIDI, Matrice de modulation, Sortir du conteneur, Choisir un patch, Écouter le player, Supprimer la colonne, Remplacer l'appareil, Réinitialiser l'appareil, Enregistrer la chaîne master comme patch..., Enregistrer le patch, Enregistrer dans la bibliothèque, Tout sélectionner dans le groupe d'appareils, Sub séparé, Égaliser le volume sonore, Générer une progression, Choisir l'accord de la roue, Envoyer la progression vers la voie d'accords, Envoyer la progression vers un clip, Ajouter une voie de notes, Ajouter une voie de motif, Ajouter un envoi de réverbération..., Ajouter une transition, Effacer l'enveloppe de gain, Forme du fondu audio, Fondus audio, Gain du clip audio, Normaliser l'audio, Normaliser l'audio à, Quantifier l'audio, Inverser l'audio, Supprimer les silences, Audio vers notes, Transposer l'audio, Warper l'audio, Warper la boucle en mesures, Mode de warp, Réglages de warp, Warper sur le tempo du morceau, Déplacer l'automation dans le clip de notes, Rendre les clips sur une nouvelle piste (sec), Rendre sur place, Rendre sur place et couper les originaux, Rendre la plage sur place, Contourner tous les players, Découper les clips en, Nettoyer l'automation, Effacer la valeur fixe, Réinitialiser tous les mutes, Effacer les voies de paramètres du clip, Réinitialiser tous les solos, Couleur du clip, Convertir la boucle, Supprimer la voie d'automation, Supprimer la voie de notes, Supprimer les événements hors des clips, Supprimer la piste et l'appareil, Détecter les accords dans l'audio, Détecter la tonalité dans l'audio, Enregistrement direct, Ducking depuis..., Choisir la source du ducking, Ducking depuis (pré-fader)..., Répartir les notes sur des voies, Étendre jusqu'au clip suivant, Étendre jusqu'au clip précédent, Chercher une répétition..., Replier les voies d'automation, Replier tous les players, Annuler le gel, Geler avec la tranche, Geler avec la queue, Geler la piste, Geler la piste jusqu'à l'appareil sélectionné, Pompage doux, Insérer des mesures aux repères, Couleur de la voie, Placer le repère gauche ici, Placer le repère droit ici, Rendre l'instance de boucle indépendante, Armement manuel de l'enregistrement, Ajouter un marqueur ici, Couleur du marqueur, Supprimer le marqueur, Placer le marqueur de fin ici, Boucler cette plage, Ajouter un marqueur de plage depuis la sélection, Renommer le marqueur..., Sélectionner les clips de la plage, Supprimer le changement de mesure, Insérer un changement de mesure, Régler la mesure, Couper la voie d'automation, Outil de notes sur les clips, Voie de motif vers notes, Chirurgien de crêtes, Refaire le rendu de la piste gelée, Supprimer les mesures aux repères, Supprimer le ducking, Renommer le clip..., Renommer le dossier..., Renommer la voie de notes..., Renommer la piste..., Inverser les clips, Réverbération inversée, Générateur de grondement, Enregistrer le clip dans la bibliothèque, Mettre le tempo à l'échelle, Envoyer les players vers la piste, Régler la valeur fixe, Régler l'aimantation, Régler le mode d'aimantation, Ajouter un point de tempo, Supprimer le point de tempo, Régler le tempo..., Rampe vers le suivant, Poursuivre les notes, Replier ou déplier le dossier, Couper la voie, Enregistrer sur la voie, Solo de la voie, Couleur de la piste, Commentaire de la piste..., Éditer les contrôleurs de la piste, Dégeler la piste, Dégeler la piste et garder l'audio, Dégrouper, Verrouillage de l'horloge, Sortie d'horloge MIDI, Envoyer l'horloge à l'arrêt, Suivre l'horloge MIDI, Suivre le tempo seulement, Ajouter un marqueur de plage depuis la boucle, Créer les voies automatiquement, Effacer la substitution, Détecter la tonalité du morceau, Quantification à l'entrée, Placer le marqueur de fin à la tête de lecture, Placer le repère gauche à la tête de lecture, Placer le repère droit à la tête de lecture, Tonalité du morceau..., Tempo au tap, Annuler le traitement vidéo, Vérifier FFmpeg, Importer une vidéo, Importer l'audio de la vidéo, Choisir la piste de l'audio vidéo, Importer l'audio avec la vidéo, Garder l'audio avec la vidéo, Déplacer la vidéo à la tête de lecture, Préparer la vidéo, Relier la vidéo, Retirer la vidéo, Incrustation vidéo, Rogner la fin de la vidéo à la tête de lecture, Rogner le début de la vidéo à la tête de lecture, Zoom de l'application, Câbles, Détacher le navigateur, Détacher le manche, Afficher la fenêtre, L'automation suit la sélection, Manche : étiquettes, Manche : gaucher, Manche : disposition d'accord, Thème suivant, Tri du rack, Actualiser la voie d'énergie, Afficher la voie d'automation, Thème, Gestionnaire de thèmes, Grand vumètre, Voie d'accords, Grille d'accords, Entraîneur d'accords, Afficher les liens de la console, Afficher la charge CPU des appareils, Voie d'énergie, Manche, Inspecteur, Voie de tonalité, Liste des marqueurs, Bandeau des marqueurs, Piano roll, Navigateur du rack, Éditeur d'échantillon, Colonne unique, Éditeur de découpe, Arrangement, Éditeur de pas, Éditeur de tablature, Voie de tempo, Panneau des pistes, Vidéo, Rangée vidéo, Afficher le morceau.

## Touches historiques d'Atomik {#atomik-legacy-keys}

### Global {#global-1}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Enregistrer | global | `Ctrl+S` | `Cmd+S` | `Ctrl+S` |
| Enregistrer sous | global | `Ctrl+Maj+S` | `Cmd+Maj+S` | `Ctrl+Maj+S` |
| Importer de l'audio | global | `Ctrl+I` | `Cmd+I` | `Ctrl+I` |
| Manuel | global | `Ctrl+F1` | `Cmd+Opt+F1` | `Ctrl+F1` |
| Mode aide | global | `Maj+F1` | `Maj+F1` | `Maj+F1` |
| Nouveau | global | `Ctrl+N` | `Cmd+N` | `Ctrl+N` |
| Ouvrir | global | `Ctrl+O` | `Cmd+O` | `Ctrl+O` |
| Quitter | global | `Ctrl+Q` | `Cmd+Q` | `Ctrl+Q` |
| Raccourcis clavier | global | `F1` | `F1` | `F1` |
| Réglages | global | `Ctrl+,` | `Cmd+,` | `Ctrl+,` |

### Transport {#transport-1}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Boucle | rack | `L`, `Maj+L` | `L`, `Maj+L` | `L`, `Maj+L` |
| Boucle | sequencer | `L`, `Maj+L` | `L`, `Maj+L` | `L`, `Maj+L` |
| Enregistrer | global | `Ctrl+Espace` | `Cmd+Espace` | `Ctrl+Espace` |
| Enregistrer | rack | `R`, `Maj+R` | `R`, `Maj+R` | `R`, `Maj+R` |
| Enregistrer | sequencer | `R`, `Maj+R` | `R`, `Maj+R` | `R`, `Maj+R` |
| Lecture / Arrêt | rack | `Espace`, `Maj+Espace` | `Espace`, `Maj+Espace` | `Espace`, `Maj+Espace` |
| Lecture / Arrêt | sequencer | `Espace`, `Maj+Espace` | `Espace`, `Maj+Espace` | `Espace`, `Maj+Espace` |
| Retour à zéro | rack | `Entrée`, `Maj+Entrée` | `Entrée`, `Maj+Entrée` | `Entrée`, `Maj+Entrée` |
| Retour à zéro | sequencer | `Entrée`, `Maj+Entrée` | `Entrée`, `Maj+Entrée` | `Entrée`, `Maj+Entrée` |

### Affichage {#view-1}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Agrandir la console | global | `Maj+F5` | `Maj+F5` | `Maj+F5` |
| Arrière du rack | global | `Tab` | `Tab` | `Tab` |
| Console | global | `F5` | `F5` | `F5` |
| Explorateur de fichiers | global | `F4` | `F4` | `F4` |
| Navigateur d'appareils | global | `Ctrl+B` | `Cmd+B` | `Ctrl+B` |
| Taille réelle | global | `Ctrl+0` | `Cmd+0` | `Ctrl+0` |
| Touches de piano | global | `F9` | `F9` | `F9` |
| Zoom arrière | global | `Ctrl+-` | `Cmd+-` | `Ctrl+-` |
| Zoom avant | global | `Ctrl+=`, `Ctrl+Maj+=`, `Ctrl++`, `Ctrl+Maj++` | `Cmd+=`, `Cmd+Maj+=`, `Cmd++`, `Cmd+Maj++` | `Ctrl+=`, `Ctrl+Maj+=`, `Ctrl++`, `Ctrl+Maj++` |

### Édition {#edit-1}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Annuler | global | `Ctrl+Z` | `Cmd+Z` | `Ctrl+Z` |
| Dupliquer | rack | `Ctrl+D` | `Cmd+D` | `Ctrl+D` |
| Dupliquer | sequencer | `Ctrl+D` | `Cmd+D` | `Ctrl+D` |
| Rétablir | global | `Ctrl+Y`, `Ctrl+Maj+Y`, `Ctrl+Maj+Z` | `Cmd+Y`, `Cmd+Maj+Y`, `Cmd+Maj+Z` | `Ctrl+Y`, `Ctrl+Maj+Y`, `Ctrl+Maj+Z` |
| Supprimer | rack | `Suppr`, `Retour arrière` | `Suppr`, `Retour arrière` | `Suppr`, `Retour arrière` |
| Supprimer | sequencer | `Suppr`, `Retour arrière` | `Suppr`, `Retour arrière` | `Suppr`, `Retour arrière` |
| Supprimer | editor | `Suppr`, `Retour arrière` | `Suppr`, `Retour arrière` | `Suppr`, `Retour arrière` |
| Tout sélectionner | sequencer | `Ctrl+A` | `Cmd+A` | `Ctrl+A` |
| Tout sélectionner | editor | `Ctrl+A` | `Cmd+A` | `Ctrl+A` |

### Rack {#rack-1}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Combiner | rack | `Ctrl+Maj+G` | `Cmd+Maj+G` | `Ctrl+Maj+G` |
| Parcourir les patchs | rack | `Ctrl+Maj+B` | `Cmd+Maj+B` | `Ctrl+Maj+B` |
| Patch précédent | rack | `Ctrl+Gauche` | `Cmd+Gauche` | `Ctrl+Gauche` |
| Patch suivant | rack | `Ctrl+Droite` | `Cmd+Droite` | `Ctrl+Droite` |
| Séparer | rack | `Ctrl+Maj+U` | `Cmd+Maj+U` | `Ctrl+Maj+U` |

### Séquenceur {#sequencer-1}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Couper les clips | sequencer | `M` | `M` | `M` |
| Couper à la tête de lecture | sequencer | `Ctrl+E` | `Cmd+E` | `Ctrl+E` |
| Décaler les clips vers la droite | sequencer | `Ctrl+Droite` | `Cmd+Droite` | `Ctrl+Droite` |
| Décaler les clips vers la droite d'un temps | sequencer | `Ctrl+Maj+Droite` | `Cmd+Maj+Droite` | `Ctrl+Maj+Droite` |
| Décaler les clips vers la droite d'un tick | sequencer | `Ctrl+Alt+Maj+Droite` | `Cmd+Opt+Maj+Droite` | `Ctrl+Alt+Maj+Droite` |
| Décaler les clips vers la gauche | sequencer | `Ctrl+Gauche` | `Cmd+Gauche` | `Ctrl+Gauche` |
| Décaler les clips vers la gauche d'un temps | sequencer | `Ctrl+Maj+Gauche` | `Cmd+Maj+Gauche` | `Ctrl+Maj+Gauche` |
| Décaler les clips vers la gauche d'un tick | sequencer | `Ctrl+Alt+Maj+Gauche` | `Cmd+Opt+Maj+Gauche` | `Ctrl+Alt+Maj+Gauche` |

### Éditeur {#editor-1}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Afficher les voies de contrôleurs | editor | `Maj+L` | `Maj+L` | `Maj+L` |
| Ajouter un marqueur au pointeur | editor | `I` | `I` | `I` |
| Couper les notes | editor | `M` | `M` | `M` |
| Décaler vers la droite | editor | `Droite` | `Droite` | `Droite` |
| Décaler vers la droite d'un temps | editor | `Maj+Droite` | `Maj+Droite` | `Maj+Droite` |
| Décaler vers la droite d'un tick | editor | `Alt+Droite` | `Opt+Droite` | `Alt+Droite` |
| Décaler vers la gauche | editor | `Gauche` | `Gauche` | `Gauche` |
| Décaler vers la gauche d'un temps | editor | `Maj+Gauche` | `Maj+Gauche` | `Maj+Gauche` |
| Décaler vers la gauche d'un tick | editor | `Alt+Gauche` | `Opt+Gauche` | `Alt+Gauche` |
| Quantifier | editor | `Q`, `Maj+Q` | `Q`, `Maj+Q` | `Q`, `Maj+Q` |
| Tablature : corde plus aiguë | tab | `Alt+Haut` | `Opt+Haut` | `Alt+Haut` |
| Tablature : corde plus grave | tab | `Alt+Bas` | `Opt+Bas` | `Alt+Bas` |
| Tablature : curseur vers le bas | tab | `Bas` | `Bas` | `Bas` |
| Tablature : curseur vers le haut | tab | `Haut` | `Haut` | `Haut` |
| Tablature : curseur à droite | tab | `Droite` | `Droite` | `Droite` |
| Tablature : curseur à gauche | tab | `Gauche` | `Gauche` | `Gauche` |
| Tablature : mesure précédente | tab | `Ctrl+Gauche` | `Cmd+Gauche` | `Ctrl+Gauche` |
| Tablature : mesure suivante | tab | `Ctrl+Droite` | `Cmd+Droite` | `Ctrl+Droite` |
| Tablature : note en triolet | tab | `Y` | `Y` | `Y` |
| Tablature : note pointée | tab | `.` | `.` | `.` |
| Tablature : technique (accent) | tab | `A` | `A` | `A` |
| Tablature : technique (effacer) | tab | `Maj+C` | `Maj+C` | `Maj+C` |
| Tablature : technique (glissé décalé) | tab | `Maj+S` | `Maj+S` | `Maj+S` |
| Tablature : technique (glissé legato) | tab | `Maj+L` | `Maj+L` | `Maj+L` |
| Tablature : technique (hammer-on) | tab | `Maj+H` | `Maj+H` | `Maj+H` |
| Tablature : technique (harmonique artificielle) | tab | `Maj+N` | `Maj+N` | `Maj+N` |
| Tablature : technique (harmonique) | tab | `N` | `N` | `N` |
| Tablature : technique (laisser sonner) | tab | `R` | `R` | `R` |
| Tablature : technique (note fantôme) | tab | `G` | `G` | `G` |
| Tablature : technique (note morte) | tab | `D` | `D` | `D` |
| Tablature : technique (note tapée) | tab | `Maj+T` | `Maj+T` | `Maj+T` |
| Tablature : technique (pré-tiré) | tab | `Alt+B` | `Opt+B` | `Alt+B` |
| Tablature : technique (pull-off) | tab | `Maj+P` | `Maj+P` | `Maj+P` |
| Tablature : technique (tiré puis relâché) | tab | `Maj+B` | `Maj+B` | `Maj+B` |
| Tablature : technique (tiré) | tab | `B` | `B` | `B` |
| Tablature : technique (vibrato large) | tab | `Maj+V` | `Maj+V` | `Maj+V` |
| Tablature : technique (vibrato) | tab | `V` | `V` | `V` |
| Tablature : technique (étouffé (paume)) | tab | `M` | `M` | `M` |
| Tablature : transposer (bas) | tab | `Ctrl+Bas` | `Cmd+Bas` | `Ctrl+Bas` |
| Tablature : transposer (haut) | tab | `Ctrl+Haut` | `Cmd+Haut` | `Ctrl+Haut` |
| Tablature : transposer (octave inférieure) | tab | `Ctrl+Maj+Bas` | `Cmd+Maj+Bas` | `Ctrl+Maj+Bas` |
| Tablature : transposer (octave supérieure) | tab | `Ctrl+Maj+Haut` | `Cmd+Maj+Haut` | `Ctrl+Maj+Haut` |
| Tablature : valeur de note (blanche) | tab | `H` | `H` | `H` |
| Tablature : valeur de note (croche) | tab | `E` | `E` | `E` |
| Tablature : valeur de note (double croche) | tab | `S` | `S` | `S` |
| Tablature : valeur de note (noire) | tab | `Q` | `Q` | `Q` |
| Tablature : valeur de note (quadruple croche) | tab | `X` | `X` | `X` |
| Tablature : valeur de note (ronde) | tab | `W` | `W` | `W` |
| Tablature : valeur de note (triple croche) | tab | `T` | `T` | `T` |
| Tablature : zoom arrière | tab | `-` | `-` | `-` |
| Tablature : zoom avant | tab | `=` | `=` | `=` |
| Tablature : étendre la sélection (bas) | tab | `Maj+Bas` | `Maj+Bas` | `Maj+Bas` |
| Tablature : étendre la sélection (droite) | tab | `Maj+Droite` | `Maj+Droite` | `Maj+Droite` |
| Tablature : étendre la sélection (gauche) | tab | `Maj+Gauche` | `Maj+Gauche` | `Maj+Gauche` |
| Tablature : étendre la sélection (haut) | tab | `Maj+Haut` | `Maj+Haut` | `Maj+Haut` |
| Transposer d'une octave vers le bas | editor | `Maj+Bas` | `Maj+Bas` | `Maj+Bas` |
| Transposer d'une octave vers le haut | editor | `Maj+Haut` | `Maj+Haut` | `Maj+Haut` |
| Transposer vers le bas | editor | `Bas` | `Bas` | `Bas` |
| Transposer vers le haut | editor | `Haut` | `Haut` | `Haut` |
| Éditeur audio : découpe | editor | `2` | `2` | `2` |
| Éditeur audio : voix | editor | `3` | `3` | `3` |
| Éditeur audio : vue d'ensemble | editor | `1` | `1` | `1` |

### Table de mixage {#mixer-1}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Aller au dernier canal | mixer | `Fin`, `Maj+Fin` | `Fin`, `Maj+Fin` | `Fin`, `Maj+Fin` |
| Aller au premier canal | mixer | `Début`, `Maj+Début` | `Début`, `Maj+Début` | `Début`, `Maj+Début` |
| Augmenter la commande active | mixer | `Haut` | `Haut` | `Haut` |
| Augmenter la commande active (fin) | mixer | `Maj+Haut` | `Maj+Haut` | `Maj+Haut` |
| Augmenter la commande active de dix | mixer | `Page préc.` | `Page préc.` | `Page préc.` |
| Augmenter la commande active de dix (fin) | mixer | `Maj+Page préc.` | `Maj+Page préc.` | `Maj+Page préc.` |
| Baisser la commande active | mixer | `Bas` | `Bas` | `Bas` |
| Baisser la commande active (fin) | mixer | `Maj+Bas` | `Maj+Bas` | `Maj+Bas` |
| Baisser la commande active de dix | mixer | `Page suiv.` | `Page suiv.` | `Page suiv.` |
| Baisser la commande active de dix (fin) | mixer | `Maj+Page suiv.` | `Maj+Page suiv.` | `Maj+Page suiv.` |
| Défiler les canaux vers la droite | mixer | `Ctrl+Droite`, `Ctrl+Maj+Droite` | `Cmd+Droite`, `Cmd+Maj+Droite` | `Ctrl+Droite`, `Ctrl+Maj+Droite` |
| Défiler les canaux vers la gauche | mixer | `Ctrl+Gauche`, `Ctrl+Maj+Gauche` | `Cmd+Gauche`, `Cmd+Maj+Gauche` | `Ctrl+Gauche`, `Ctrl+Maj+Gauche` |
| Sélectionner le canal précédent | mixer | `Gauche` | `Gauche` | `Gauche` |
| Sélectionner le canal suivant | mixer | `Droite` | `Droite` | `Droite` |
| Étendre la sélection vers la droite | mixer | `Maj+Droite` | `Maj+Droite` | `Maj+Droite` |
| Étendre la sélection vers la gauche | mixer | `Maj+Gauche` | `Maj+Gauche` | `Maj+Gauche` |

### Fichiers {#files-1}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Arrêter l'aperçu | files | `Échap` | `Échap` | `Échap` |
| Basculer le favori | files | `F` | `F` | `F` |
| Chercher des fichiers | files | `Ctrl+F` | `Cmd+F` | `Ctrl+F` |
| Dernier élément | files | `Fin` | `Fin` | `Fin` |
| Dossier parent | files | `Retour arrière`, `Alt+Haut` | `Retour arrière`, `Opt+Haut` | `Retour arrière`, `Alt+Haut` |
| Déplier | files | `Droite` | `Droite` | `Droite` |
| Lecture / arrêt de l'aperçu | files | `Espace` | `Espace` | `Espace` |
| Ouvrir | files | `Entrée` | `Entrée` | `Entrée` |
| Page précédente | files | `Page préc.` | `Page préc.` | `Page préc.` |
| Page suivante | files | `Page suiv.` | `Page suiv.` | `Page suiv.` |
| Premier élément | files | `Début` | `Début` | `Début` |
| Renommer l'élément de la bibliothèque | files | `F2` | `F2` | `F2` |
| Replier | files | `Gauche` | `Gauche` | `Gauche` |
| Supprimer l'élément de la bibliothèque | files | `Suppr` | `Suppr` | `Suppr` |
| Sélectionner tous les fichiers | files | `Ctrl+A` | `Cmd+A` | `Ctrl+A` |
| Élément précédent | files | `Haut` | `Haut` | `Haut` |
| Élément suivant | files | `Bas` | `Bas` | `Bas` |

### Touches de piano {#piano-keys-1}

| Commande | Où | Windows | macOS | Linux |
|---|---|---|---|---|
| Jouer une note (do (haut)) | keys | `K` | `K` | `K` |
| Jouer une note (do dièse (haut)) | keys | `O` | `O` | `O` |
| Jouer une note (do dièse) | keys | `W` | `W` | `W` |
| Jouer une note (do) | keys | `A` | `A` | `A` |
| Jouer une note (fa (haut)) | keys | `'`, `"` | `'`, `"` | `'`, `"` |
| Jouer une note (fa dièse) | keys | `T` | `T` | `T` |
| Jouer une note (fa) | keys | `F` | `F` | `F` |
| Jouer une note (la dièse) | keys | `U` | `U` | `U` |
| Jouer une note (la) | keys | `H` | `H` | `H` |
| Jouer une note (mi (haut)) | keys | `;`, `:` | `;`, `:` | `;`, `:` |
| Jouer une note (mi) | keys | `D` | `D` | `D` |
| Jouer une note (ré (haut)) | keys | `L` | `L` | `L` |
| Jouer une note (ré dièse (haut)) | keys | `P` | `P` | `P` |
| Jouer une note (ré dièse) | keys | `E` | `E` | `E` |
| Jouer une note (ré) | keys | `S` | `S` | `S` |
| Jouer une note (si) | keys | `J` | `J` | `J` |
| Jouer une note (sol dièse) | keys | `Y` | `Y` | `Y` |
| Jouer une note (sol) | keys | `G` | `G` | `G` |
| Octave inférieure | keys | `Z` | `Z` | `Z` |
| Octave supérieure | keys | `X` | `X` | `X` |
| Régler la vélocité (1) | keys | `1` | `1` | `1` |
| Régler la vélocité (10) | keys | `0` | `0` | `0` |
| Régler la vélocité (2) | keys | `2` | `2` | `2` |
| Régler la vélocité (3) | keys | `3` | `3` | `3` |
| Régler la vélocité (4) | keys | `4` | `4` | `4` |
| Régler la vélocité (5) | keys | `5` | `5` | `5` |
| Régler la vélocité (6) | keys | `6` | `6` | `6` |
| Régler la vélocité (7) | keys | `7` | `7` | `7` |
| Régler la vélocité (8) | keys | `8` | `8` | `8` |
| Régler la vélocité (9) | keys | `9` | `9` | `9` |

Commandes sans touche dans ce profil (les menus et les Réglages y mènent toujours) : Canaux d'entrée, Canaux de sortie, Plugins, Réanalyser les ports MIDI, Redémarrer l'audio, Séparer les stems, Effacer l'automation, Éditer l'automation, Régler la valeur d'automation fixe, Annuler la recherche, Créer l'appareil sélectionné, Navigateur : bas, Navigateur : haut, Calibrer la rotation de la caméra, Aperçu de la caméra, Caméra, Boutons du contrôleur : page suivante, Boutons du contrôleur : page précédente, Créer une piste audio, Créer un appareil, Créer un effet, Créer un instrument, Créer un canal de mixage, Créer un player, Copier, Copier les réglages, Couper, Historique d'annulation, Coller, Coller l'accord, Coller les réglages, Options de quantification, Basculer entre piano roll et tablature, Appliquer la courbe aux notes sélectionnées, Arpéger la sélection, Ajouter des marqueurs aux transitoires, Rétablir les marqueurs, Couper le clip aux marqueurs, Clip vers le découpeur de boucles, Écouter les notes, Découper les notes, Transformer la sélection en accords, Caler la sélection sur la gamme, Détecter la gamme d'après les notes, Éditer l'expression des notes, Étendre la sélection à la note suivante, Étendre la sélection à la note précédente, Extraire les notes vers une nouvelle voie, Ajuster les notes aux accords, Régler le repli, Replier, Suivre la tonalité du morceau, Joindre les notes, Rendre legato, Outil de notes, Aimanter à la gamme, Sélectionner la note suivante, Sélectionner la note précédente, Sélectionner les notes de même hauteur, Régler la durée des notes de l'arpège, Régler les octaves de l'arpège, Régler le motif de l'arpège, Choisir l'accord, Choisir le renversement, Choisir la gamme, Choisir la fondamentale, Régler l'amplitude du strum, Régler le sens du strum, Appliquer la transposition à la grille, Grille : défilement automatique, Grille : capodastre, Supprimer la grille d'accords, Modifier la grille d'accords en texte, Grille : taille du texte, Nouvelle grille d'accords, Renommer la grille d'accords, Choisir la grille d'accords, Grille : dièses ou bémols, Grille : transposer l'affichage, Modifier les pas, Inverser les pas, Pas aléatoires, Décaler les pas, Strummer la sélection, Tablature : amplitude du tiré, Tablature : changer le doigté, Tablature : instrument, accordage, capo, Alléger les points de contrôleur, Entraîneur d'accords : temps par accord, Entraîneur d'accords : tempo, Entraîneur d'accords : capodastre, Entraîneur d'accords : saisir une liste d'accords, Entraîneur d'accords : instrument, Entraîneur d'accords : tonalité, Entraîneur d'accords : mode d'exercice, Entraîneur d'accords : ordre des accords, Entraîneur d'accords : accord de la paire, Entraîneur d'accords : réinitialiser les scores, Entraîneur d'accords : septièmes, Entraîneur d'accords : passer l'accord, Entraîneur d'accords : origine des accords, Entraîneur d'accords : démarrer ou arrêter, Entraîneur d'accords : accordage suivant, Entraîneur d'accords : doigté, Corriger la hauteur des notes, Aplatir la dérive des notes, Réinitialiser les retouches de hauteur, Corriger selon la tonalité du morceau, Touches plus hautes, Touches plus courtes, Réinitialiser la hauteur des touches, Annuler l'archive du projet, Annuler la collecte ou le déplacement, Vider les morceaux récents, Fermer, Rassembler les fichiers, Aperçu de la collecte des fichiers, Convertir la fréquence d'échantillonnage, Réinitialiser le morceau par défaut, Utiliser le morceau comme morceau par défaut, Cibles de livraison, Exporter l'archive du projet, Exporter la liste des paramètres de l'appareil, Exporter le film avec le mix, Exporter pour un usage, Exporter la boucle en fichier audio, Exporter le clip MIDI, Exporter le morceau en MIDI, Exporter la piste MIDI, Exporter le patch, Exporter la sélection en fichier audio, Exporter la grille d'accords en PDF, Exporter la grille d'accords en texte, Exporter le morceau en fichier audio, Exporter les stems en fichiers audio, Exporter la tablature en MusicXML, Exporter la tablature en PDF, Exporter la tablature en texte, Importer une archive de projet, Importer du MIDI, Importer le MIDI comme morceau, Importer une grille d'accords, Importer un fichier de tablature ou de partition, Importer la tablature ou la partition comme morceau, Détecter les accords à l'import MIDI, Résolution de l'export MIDI, Déplacer le projet, Nouveau à la fréquence, Nouveau depuis un modèle, Ouvrir un récent, Relier l'audio manquant, Restaurer une sauvegarde, Enregistrer comme modèle, Enregistrer une version, Partager le projet, Afficher le dossier des modèles, Informations du morceau, Analyste du son, Écran de démarrage, Rack de stress, Morceau de stress, Versions, À propos d'Atomik Rack, Rechercher des mises à jour, Documentation, Exporter le diagnostic, Visite guidée, Signaler un problème, Retour à l'arrangement, Clip vers le lanceur, Lancer le clip, Lancer la scène, Nouvelle scène, Quantification du lancement, Enregistrer les lancements, Arrêter tous les clips, La scène arrête les pistes vides, Clip du lanceur vers l'arrangement, Assigner à une macro, Assigner à une nouvelle banque de macros, Nouvelle banque de macros, Macros..., Retirer de la macro, Effacer les assignations MIDI, Exporter un préréglage d'assignations MIDI..., Oublier le MIDI, Importer un préréglage d'assignations MIDI..., Apprentissage MIDI, Apprentissage MIDI, Verrouiller les surfaces sur la sélection, Assigner une commande, Assignations MIDI..., Ajouter un canal, Ajouter une piste de référence..., Ajouter à un VCA, La basse sera-t-elle audible ?..., Contourner tous les inserts, Vérifier le mix..., Vider les inserts, Comparer A/B, Copier les réglages du canal, Copier les inserts du canal, Copier les niveaux du canal, Copier le mute et le solo du canal, Copier les envois du canal, Créer un canal parallèle, Créer un VCA depuis la sélection, Créer un VCA depuis le dossier, Supprimer l'instantané, Supprimer le VCA, Terminer la comparaison, Contrôle kick et basse, Sub de club, Casque, Mono, Petite enceinte, Lier les faders, Verrouiller / déverrouiller le canal, Carte de masquage..., Mode du vumètre, Morpher vers l'instantané précédent, Morpher vers l'instantané suivant, Durée du morphing, Piste de référence suivante, Coller les réglages du canal, Rappeler l'instantané suivant, Rappeler l'instantané précédent, Rappeler l'instantané, Bande de référence, Début de la référence, Caler sur le niveau de référence, Panneau de référence..., Solo stéréo de la référence, Supprimer le canal, Retirer du VCA, Retirer la piste de référence, Réinitialiser le canal, Router le dossier vers un nouveau bus, Router vers un nouveau bus de sortie, Router le groupe vers un nouveau bus, Sélectionner la piste de référence, Afficher le phasemètre, Afficher le spectre, L'instantané inclut les effets, Mémoriser l'instantané, Compensation de latence, Inverser la polarité, Écouter la référence, Aligner les retours, Les envois suivent le bus, Arrêter le routage du dossier, Arrêter le routage du groupe, Mettre à jour l'instantané, Configuration audio et MIDI, Gestionnaire de plugins, Apprendre l'automation depuis la fenêtre du plugin, Apprendre le MIDI depuis la fenêtre du plugin, Passer le chargement du plugin, Pratique : décompte, Pratique : écouter et noter, Pratique : calibrer la latence d'écoute, Pratique : effacer les résultats, Pratique : entrée d'écoute suivante, Pratique : décalage d'écoute, Pratique : précision d'un passage propre, Pratique : boucler la sélection, Pratique : vitesse, Pratique : plus lent, Pratique : plus rapide, Pratique : entraîneur de vitesse, Pratique : passages par palier, Pratique : vitesse de départ, Pratique : palier de l'entraîneur, Pratique : vitesse visée, Ajouter une colonne, Ajouter un effet d'insert..., Ajouter un effet d'envoi..., Utiliser comme effet d'envoi, Routage auto, Test A/B à l'aveugle..., Nettoyer la basse, Déconnecter, Étendre la sélection vers le bas, Étendre la sélection vers le haut, Aller à la piste, Importer les données du casque, Choisir le fichier de données du casque, Retirer les données du casque, Brancher la prise, Charger un patch de chaîne master..., Choisir le port de sortie MIDI, Choisir le port de sortie MIDI, Matrice de modulation, Sortir du conteneur, Choisir un patch, Écouter le player, Supprimer la colonne, Remplacer l'appareil, Réinitialiser l'appareil, Enregistrer la chaîne master comme patch..., Enregistrer le patch, Enregistrer dans la bibliothèque, Tout sélectionner dans le groupe d'appareils, Sélectionner l'appareil suivant, Sélectionner l'appareil précédent, Sub séparé, Égaliser le volume sonore, Générer une progression, Choisir l'accord de la roue, Envoyer la progression vers la voie d'accords, Envoyer la progression vers un clip, Ajouter une voie de notes, Ajouter une voie de motif, Ajouter un envoi de réverbération..., Ajouter une transition, Fondu enchaîné audio, Effacer l'enveloppe de gain, Forme du fondu audio, Fondus audio, Gain du clip audio, Normaliser l'audio, Normaliser l'audio à, Quantifier l'audio, Inverser l'audio, Supprimer les silences, Audio vers notes, Transposer l'audio, Warper l'audio, Warper la boucle en mesures, Mode de warp, Réglages de warp, Warper sur le tempo du morceau, Déplacer l'automation dans le clip de notes, Rendre les clips sur une nouvelle piste (sec), Rendre sur place, Rendre sur place et couper les originaux, Rendre la plage sur place, Contourner tous les players, Découper les clips en, Nettoyer l'automation, Effacer la valeur fixe, Réinitialiser tous les mutes, Effacer les voies de paramètres du clip, Réinitialiser tous les solos, Couleur du clip, Fermer le clip, Convertir la boucle, Supprimer la voie d'automation, Supprimer la voie de notes, Supprimer les événements hors des clips, Supprimer la piste et l'appareil, Détecter les accords dans l'audio, Détecter la tonalité dans l'audio, Enregistrement direct, Ducking depuis..., Choisir la source du ducking, Ducking depuis (pré-fader)..., Répartir les notes sur des voies, Étendre jusqu'au clip suivant, Étendre jusqu'au clip précédent, Étendre la sélection de pistes vers le bas, Étendre la sélection de pistes vers le haut, Chercher une répétition..., Replier les voies d'automation, Replier tous les players, Suivre la tête de lecture, Annuler le gel, Geler avec la tranche, Geler avec la queue, Geler la piste, Geler la piste jusqu'à l'appareil sélectionné, Pompage doux, Grouper les pistes, Insérer des mesures aux repères, Joindre, Couleur de la voie, Placer le repère gauche ici, Placer le repère droit ici, Rendre l'instance de boucle indépendante, Armement manuel de l'enregistrement, Ajouter un marqueur ici, Couleur du marqueur, Supprimer le marqueur, Placer le marqueur de fin ici, Boucler cette plage, Ajouter un marqueur de plage depuis la sélection, Renommer le marqueur..., Sélectionner les clips de la plage, Fusionner les voies de notes, Supprimer le changement de mesure, Insérer un changement de mesure, Régler la mesure, Descendre la piste, Monter la piste, Couper la voie d'automation, Outil de notes sur les clips, Ouvrir le clip, Voie de motif vers notes, Chirurgien de crêtes, Refaire le rendu de la piste gelée, Supprimer les mesures aux repères, Supprimer le ducking, Renommer le clip..., Renommer le dossier..., Renommer la voie de notes..., Renommer la piste..., Inverser les clips, Réverbération inversée, Générateur de grondement, Enregistrer le clip dans la bibliothèque, Mettre le tempo à l'échelle, Sélectionner le clip suivant, Sélectionner la piste suivante, Sélectionner le clip précédent, Sélectionner la piste précédente, Envoyer les players vers la piste, Régler la valeur fixe, Régler l'aimantation, Régler le mode d'aimantation, Aimantation activée/désactivée, Ligne de prise suivante, Jouer la ligne de prise seule, Ligne de prise précédente, Ajouter un point de tempo, Supprimer le point de tempo, Régler le tempo..., Rampe vers le suivant, Poursuivre les notes, Replier ou déplier le dossier, Couper la voie, Enregistrer sur la voie, Solo de la voie, Couleur de la piste, Commentaire de la piste..., Éditer les contrôleurs de la piste, Hauteur de la piste, Dégeler la piste, Dégeler la piste et garder l'audio, Dégrouper, Zoomer sur la sélection, Pistes plus hautes, Pistes plus basses, Réinitialiser la hauteur des pistes, Verrouillage de l'horloge, Sortie d'horloge MIDI, Envoyer l'horloge à l'arrêt, Suivre l'horloge MIDI, Suivre le tempo seulement, Outil gomme, Outil main, Outil loupe, Outil mute, Outil crayon, Outil rasoir, Outil de sélection, Outil enceinte, Ajouter un marqueur, Ajouter un marqueur de plage depuis la boucle, Toutes les notes coupées, Alt : nouveau calque, couper le dernier, Créer les voies automatiquement, Reculer d'une mesure, Avancer d'une mesure, Effacer la substitution, Détecter la tonalité du morceau, Overdub : nouveau calque, Avancer d'une mesure, Aller au repère, Quantification à l'entrée, Boucler la sélection et lire, Déplacer la boucle, Boucle sur la sélection, Métronome, Marqueur suivant, Lecture, Décompte, Marqueur précédent, Reculer d'une mesure, Placer le marqueur de fin à la tête de lecture, Placer le repère gauche à la tête de lecture, Placer le repère droit à la tête de lecture, Tonalité du morceau..., Arrêt, Tempo au tap, Tempo plus lent, Tempo plus rapide, Annuler le traitement vidéo, Vérifier FFmpeg, Importer une vidéo, Importer l'audio de la vidéo, Choisir la piste de l'audio vidéo, Importer l'audio avec la vidéo, Garder l'audio avec la vidéo, Déplacer la vidéo à la tête de lecture, Préparer la vidéo, Relier la vidéo, Retirer la vidéo, Incrustation vidéo, Rogner la fin de la vidéo à la tête de lecture, Rogner le début de la vidéo à la tête de lecture, Zoom de l'application, Câbles, Changer l'affichage des câbles, Palette de commandes..., Détacher le navigateur, Détacher la table de mixage, Détacher le manche, Détacher le rack, Chercher un paramètre..., Aller à la recherche, Afficher la fenêtre, L'automation suit la sélection, Manche : étiquettes, Manche : gaucher, Manche : disposition d'accord, Thème suivant, Tri du rack, Actualiser la voie d'énergie, Séquenceur seul, Afficher la voie d'automation, Thème, Gestionnaire de thèmes, Grand vumètre, Voie d'accords, Grille d'accords, Entraîneur d'accords, Afficher les liens de la console, Afficher la charge CPU des appareils, Éditeur de clip, Voie d'énergie, Manche, Inspecteur, Voie de tonalité, Lanceur, Liste des marqueurs, Bandeau des marqueurs, Piano roll, Rack, Navigateur du rack, Éditeur d'échantillon, Colonne unique, Éditeur de découpe, Arrangement, Égaliseur à spectre, Éditeur de pas, Éditeur de tablature, Voie de tempo, Panneau des pistes, Vidéo, Rangée vidéo, Fermer la fenêtre du morceau, Nouvelle fenêtre de morceau, Morceau suivant, Ouvrir le morceau dans une nouvelle fenêtre, Morceau précédent, Afficher le morceau.
