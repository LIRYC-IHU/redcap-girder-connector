# Pseudonymisation des fichiers : signature du NIP, dates et identifiants

Statut : **proposé** — à relire par le DPO et un relecteur compétent en cryptographie
avant mise en production. Référence : annexe sécurité des méthodologies de référence de la
CNIL, version avril 2026 (notée MR-… ci-dessous).

## 1. Objectifs

Le module envoie vers Girder des fichiers (DICOM, ECG XML aECG, Holter Schiller)
désidentifiés dans le navigateur. Trois besoins s'ajoutent à la désidentification :

1. **Contrôle qualité** : détecter qu'un envoi rattache à un record REDCap des examens
   d'un autre patient. Les envois successifs d'un record peuvent venir de navigateurs, de
   postes et d'utilisateurs différents, et **le REDCap ne contient aucun identifiant du
   patient**.
2. **Dates** : conserver la séquence et les intervalles exacts entre les examens d'un
   patient, sans que les dates réelles ni l'âge exact puissent être retrouvés.
3. **Identifiants techniques** : qu'un UID ou un numéro d'origine ne permette pas de
   relier un fichier à sa source sans clé secrète (MR-CNS-03/04).

Contraintes :

- le NIP ne quitte pas le navigateur, et la clé secrète ne quitte pas le serveur REDCap
  (MR-SEC-05, MR-CNS-04, MR-SEC-09) ;
- le détenteur de la clé ne doit pas pouvoir remonter à un NIP par force brute ;
- les destinataires des données (utilisateurs Girder) ne doivent pouvoir retrouver ni
  l'identité, ni les dates réelles, ni l'âge exact.

## 2. Principe : un OPRF, et une sortie qui ne remonte à rien

Tout dérive d'une seule valeur : `y = F_k(NIP)`, où `F` est un **OPRF vérifiable**
(RFC 9497, suite `ristretto255-SHA512`, mode VOPRF) et `k` la clé de l'installation
REDCap. C'est une fonction de hachage paramétrée par une clé secrète, au sens de
MR-CNS-03, calculée sans que le NIP aille au serveur ni que la clé aille au navigateur :

- le navigateur masque le NIP par un aléa : le serveur ne voit qu'un point aléatoire ;
- le serveur applique `k` et prouve (preuve DLEQ) qu'il a utilisé la clé qu'il publie ;
- le navigateur retire l'aléa et obtient `y`, qui ne quitte pas le navigateur.

**La protection contre le détenteur de la clé vient de la troncature.** L'espace des NIP
est petit : avec `k`, on peut calculer `F_k` pour tous les NIP. Mais REDCap ne stocke
qu'**un octet** dérivé de `y`. Chaque valeur correspond à environ 1/256 des NIP, soit des
millions de candidats : l'énumération n'aboutit à rien.

## 3. Protocole

### 3.1 Entrée

```
x = "liryc-pseudonym-v1" ‖ 0x00 ‖ domaine ‖ 0x00 ‖ NIP normalisé
```

- **domaine** : réglage de projet `pseudonym-domain`, initialisé à `redcap:<pid>`. Il
  sépare les projets. Modifiable (migration d'un projet), mais **jamais après le premier
  envoi**.
- **NIP normalisé** : espaces retirés, majuscules, zéros de tête retirés s'il est
  entièrement numérique. Vide après normalisation ⇒ NIP manquant.

### 3.2 Échanges

```
Navigateur                                   REDCap (clé k)
   r ← aléatoire
   B = r · H(x)
   ── pseudonym-evaluate(B) ───────────────▶ limite de débit, journal
                                             Z = k · B, preuve π
   ◀───────────────────────── Z, π, K
   vérifie π
   N = r⁻¹ · Z = k · H(x)
   y = Finalize(x, N)                        (SHA-512, 64 octets)
   signature = SHA-512("liryc-pseudonym-fingerprint-v1" ‖ y)[0..1]
   graine    = SHA-512("liryc-pseudonym-date-offset-v1" ‖ y)[0..8]
   clé UID   = SHA-512("liryc-pseudonym-uid-key-v1" ‖ y ‖ record_id)[0..32]
   ── pseudonym-check(signature) ──────────▶ Girder joignable ? comparaison, envois existants,
                                             rien de stocké
   fenêtre de revue, décision de l'utilisateur
   ── pseudonym-register(signature) ───────▶ stockage si première fois, alerte si discordance
                                             validée, ticket
```

Seule la signature est transmise. Le décalage et la clé UID restent dans le navigateur.

Deux implémentations : la crate Rust `crates/liryc-pseudonym` (sur `curve25519-dalek`,
compilée en WASM) pour le navigateur, et PHP sur le ristretto255 natif de libsodium pour
REDCap. libsodium 1.0.18 accepte deux encodages que la RFC 9496 refuse (l'identité et le
bit 255 à 1) : le code PHP les rejette explicitement. Toutes deux sont validées contre les
vecteurs de la RFC 9497 (annexe A.1.2), contre les vecteurs du protocole
(`protocol/test-vectors/pseudonym.json`) et contre 625 encodages dont le verdict RFC est
fixé.

### 3.3 Contrôle qualité

Dans l'ordre, avant tout envoi :

1. **Résumé.** Le navigateur lit chaque fichier (pour le DICOM, l'en-tête seulement,
   jusqu'aux pixels) et construit un résumé du transfert : NIP tel qu'écrit dans les
   fichiers, nom, prénom, date de naissance ; pour le DICOM, le nombre de séries avec leur
   modalité, leur description et leur nombre d'images ; le nombre d'ECG et de Holter. Ce
   résumé reste dans le navigateur.
2. **Contrôle de la signature**, sans rien stocker : le pseudonyme est calculé (§3.2) et
   `pseudonym-check` vérifie que Girder répond, puis compare la signature à celle du
   record, s'il en a une.
3. **Revue** : une fenêtre REDCap (la `dialog` jQuery UI de l'Online Designer) montre nom,
   prénom, date de naissance, NIP, le type des données (séries DICOM avec leur modalité,
   ECG, Holter) et le résultat du contrôle. Tout ce qui vient d'un fichier y entre comme
   texte, jamais comme HTML, et la fenêtre est retirée de la page à sa fermeture.
4. **Si l'utilisateur valide**, `pseudonym-register` stocke la signature si le record n'en
   a pas encore — en la comparant de nouveau, sous verrou, car un autre envoi a pu en
   stocker une entre-temps — et délivre le ticket ; les fichiers sont ensuite désidentifiés
   et envoyés.

| Situation | Résultat |
|---|---|
| Girder injoignable | **refus avant toute confirmation** : sinon la signature serait stockée sans contenu pour l'effacer |
| Aucune signature, aucun envoi dans Girder | revue « premier envoi » ; signature stockée après validation |
| Aucune signature, envois déjà présents | revue « comparaison impossible » : ils datent d'une version sans signature ; signature stockée après validation |
| Signature identique | revue « même signature », avec le nombre d'envois déjà présents |
| Signature différente | **alerte rouge** ; l'utilisateur peut annuler ou valider malgré tout (« Upload anyway ») |
| Discordance validée | envoi effectué ; le record garde sa signature ; alerte tracée dans le journal du projet (*Logging*), dans celui du module et sur le champ, où le widget continue de l'afficher |
| Discordance apparue après la validation (envoi simultané) | **refus** : l'utilisateur ne l'a pas vue ; il recommence et la voit |
| L'utilisateur annule | rien n'est stocké ni envoyé, le champ et le formulaire restent inchangés (une évaluation a eu lieu : elle compte dans la limite de débit) |

**Ce que vaut la signature.** Elle dit que deux envois proviennent *a priori* du même
patient. Une erreur passe inaperçue dans 1 cas sur 256, ce qui suffit contre des erreurs
de manipulation, pas contre un acte volontaire. Elle ne sert pas d'index (MR-CNS-01), et
ne permet pas de repérer un doublon entre records. Pour qui détient la clé et le NIP d'une
personne, elle écarte 255 records sur 256 : cette protection dépend donc de la taille de
la cohorte.

Le ticket est signé (HMAC, valable 1 h) et lié au record, à la signature et au fait que
la discordance ait été validée ou non ; `init-batch` le refuse s'il manque dès lors que le
lot contient des fichiers désidentifiés. La signature
est stockée dans un réglage interne du module, jamais dans le champ du formulaire (elle
partirait dans les exports), jamais dans Girder.

Une signature enregistrée par erreur est effacée quand on supprime le contenu du record
(action existante) et que plus aucun envoi ne lui est rattaché.

**Le contrôle est optionnel** : la case de projet `pseudonym-fingerprint-check` (cochée par
défaut) l'active. Décochée, aucune signature n'est stockée ni comparée et `init-batch`
n'exige pas de ticket. L'évaluation reste faite, car elle fournit le décalage et la clé
UID : un NIP manquant reste bloquant, et un lot mêlant plusieurs NIP reste refusé. Les
signatures déjà stockées sont conservées et de nouveau comparées si la case est recochée.

Le navigateur écarte par ailleurs, avant tout appel réseau :

| Situation | Résultat |
|---|---|
| Fichier à désidentifier sans NIP | ce fichier est mis de côté (motif : NIP manquant), le reste du lot continue |
| Plusieurs NIP distincts dans le lot | **le lot entier est refusé** |

### 3.4 Dates

Le décalage, en jours, est dérivé de la graine :

```
décalage = −(18 262 + (graine mod 18 263))     ∈ [−36 524, −18 262], soit 50 à 100 ans
```

Il est calculé et appliqué **dans le navigateur** et n'est stocké nulle part.

- **Toutes** les dates (DICOM `DA`/`DT`, séquences comprises ; dates des ECG XML) sont
  décalées de ce nombre de jours, **dans leurs champs d'origine**. Il dépend du seul
  patient : les intervalles entre ses examens sont exacts, quels que soient le format,
  l'envoi ou le record.
- Les dates tombent au XXᵉ siècle : manifestement fictives, elles ne révèlent ni l'époque,
  ni la saison, ni le jour de la semaine.
- **La date de naissance est vidée** (DICOM : élément présent et vide, l'attribut étant de
  type 2). Décalée comme les autres, elle redonnerait l'âge exact par soustraction.
- **L'âge à l'examen** est écrit dans `PatientAge` (0010,1010), calculé sur les dates
  réelles avant décalage : en années (`067Y`), en mois avant 2 ans (`018M`), en semaines
  avant 2 mois (`006W`). Sans date de naissance, un `PatientAge` source valide est
  conservé, sinon il est absent.
- DICOM : `LongitudinalTemporalInformationModified` (0028,0303) ← `MODIFIED`, et
  `DeidentificationMethod` porte la version du module.
- Une valeur illisible comme date est supprimée ; un élément de date non typé comme tel
  (`UN`) est supprimé.

**Pourquoi dériver le décalage plutôt que le tirer au hasard et le stocker.** Un décalage
stocké donnerait à l'administrateur REDCap, qui a aussi accès à Girder, les vraies dates de
tous les records. Dérivé, il exige le NIP. Il n'y a pas non plus de table à protéger, et un
patient présent dans deux records garde un seul calendrier. La contrepartie : pour qui
détient la clé, un fichier et une vraie date d'examen de ce fichier, le décalage donne
environ 14 bits de plus sur le NIP. Mais cela suppose de connaître déjà cette date, donc
très probablement le patient.

### 3.5 Identifiants techniques DICOM

L'anonymiseur remplace les UID et l'`AccessionNumber` par un hachage **sans clé** : toute
personne ayant accès à l'original (le PACS) pourrait refaire le lien, ce qu'exclut
MR-CNS-04. Ces valeurs sont donc traitées avant lui :

- **UID** : `{racine Liryc}.{120 bits de HMAC-SHA256(clé UID, UID d'origine)}`. Une seule
  évaluation suffit pour tout le lot, la clé UID se déduisant de `y` dans le navigateur.
  Un même UID donne toujours le même résultat dans un record : les séries restent groupées
  et les références restent valides. La clé dépend du `record_id` : une même étude dans
  deux records n'est pas reliable par ses UID. Les UID définis par le standard
  (`1.2.840.10008.*`) et les classes SOP sont laissés tels quels.
- **`AccessionNumber` et numéros de commande d'imagerie** : présents et vides.

## 4. Qui sait quoi

| Acteur | Détient | Peut |
|---|---|---|
| Navigateur de l'utilisateur | les fichiers originaux, `y`, le décalage, la clé UID | rien de plus que ce que l'utilisateur sait déjà |
| Administrateur REDCap | la clé `k`, les signatures, la clé API Girder | pour un NIP qu'il connaît : calculer signature, décalage et clé UID, donc tester un record et, s'il tombe sur le bon, en lire les vraies dates. Sans NIP : rien |
| Administrateur Girder | les fichiers désidentifiés | rien : ni clé, ni signature, ni décalage |
| Destinataire des données | les fichiers désidentifiés | rien |

## 5. Limites

- **Le code client est servi par REDCap.** Un administrateur REDCap *malveillant* peut
  modifier le JavaScript ou le WASM pour exfiltrer le NIP. Aucune cryptographie n'y
  remédie ; le schéma protège contre un administrateur honnête mais curieux. Mesures
  organisationnelles : code relu, paquet publié avec empreinte, déploiement contrôlé.
- **La clé est irremplaçable** : la changer ou la perdre change toutes les signatures
  (fausses alertes au contrôle qualité), tous les décalages (calendriers rompus) et tous les
  UID. Elle se sauvegarde comme une clé de chiffrement, et une politique de gestion est à
  rédiger (MR-SEC-09, MR-CNS-03). Même chose pour le domaine.
- **NIP hétérogènes** : selon le système, le champ « identifiant » peut porter l'IPP, un
  numéro de séjour ou une saisie manuelle. Le contrôle le signale par une alerte. Si elle
  est validée, ou si le contrôle est décoché, deux NIP différents dans un record donnent
  deux calendriers.
- **Holter Schiller** : la date d'enregistrement n'est pas décalée (son emplacement dans
  le format n'est pas documenté). Lacune existante.
- **Documents encapsulés** (PDF, CDA) : un fichier DICOM qui en contient est refusé et mis
  de côté avec son motif, car l'identité est dans le document, hors de portée. Les comptes
  rendus ne passent donc pas par ce module.
- **Secondary Capture** : les cinq classes sont refusées de la même façon. Une capture
  d'écran ou de document peut montrer l'identité dans ses pixels, que le module ne lit pas.
  Le refus ne porte que sur ces classes : le contrôle du texte incrusté dans les autres
  images reste fait en amont.
- **Texte incrusté dans les images** (MR-SEC-07) : le module ne traite pas les pixels. Son
  absence est vérifiée en amont, ce que le PIA doit décrire, avec l'analyse des
  particularités visuelles (dispositifs implantés visibles en scopie, par exemple).
- **`record_id`** (MR-CNS-02) : il est écrit dans les fichiers et sert de dossier Girder. Il
  doit être un code non signifiant, généré aléatoirement par un module REDCap dédié, et non
  un numéro d'inclusion séquentiel.

## 6. Champ lu comme NIP

| Format | Champ, par ordre de préférence |
|---|---|
| DICOM | `PatientID` (0010,0020) |
| ECG XML (aECG) | `trialSubject/id@extension`, à défaut un `PatientID` constructeur |
| Holter Schiller | champ « patient id » (offset `0x0B2B`, 28 octets, encodé par XOR) |

Le guide d'implémentation HL7 aECG place l'identifiant du sujet dans `trialSubject/id`
(« the traditional identifier » dans `extension`) ; `PatientID` n'en fait pas partie. Sur
l'ECG réel de test (Mindray BeneHeart R12), ces deux champs portent **deux identifiants
différents** : il reste à vérifier que `extension` est bien l'IPP.

## 7. Composants

| Composant | Rôle |
|---|---|
| `crates/liryc-pseudonym` | l'OPRF en Rust : masquage, vérification, finalisation, dérivations ; évaluation pour les tests et les vecteurs |
| `wasm/dicom_deid` (DICOM, XML, Schiller) | lecture du NIP et du résumé ; dates, UID et identifiants |
| `src/Voprf.php`, `src/GirderUploaderModule.php` | clé `k`, évaluation + preuve, limite de débit, verrou, stockage, ticket |
| `src/js/` | inspection, résumé, contrôle, confirmation, puis désidentification |
| `protocol/test-vectors/` | vecteurs RFC 9497, vecteurs du protocole, encodages |
| `tests/e2e/` | pile Docker (REDCap, MariaDB, Girder, MongoDB) et scénarios Playwright |

Réglages :

| Réglage | Portée | Rôle |
|---|---|---|
| `pseudonym-redcap-key` | système (Control Center) | la clé `k`, 32 octets hex ; générée si vide ; jamais modifiée après usage |
| `pseudonym-daily-limit` | système | évaluations par 24 h et par projet (défaut 500) |
| `pseudonym-domain` | projet | domaine de séparation (défaut `redcap:<pid>`) |
| `pseudonym-fingerprint-check` | projet | case à cocher : contrôle de la signature (défaut : cochée) |

## 8. Tests de bout en bout

1. NIP manquant ⇒ fichier mis de côté avec son motif ; si plus rien ne reste, rien n'est
   envoyé.
2. Premier envoi du record ⇒ la fenêtre de revue montre le résumé et « premier envoi » ; après
   validation, signature stockée ; dans Girder, dates décalées, date de naissance vide,
   âge en années.
3. Envoi suivant, même patient ⇒ accepté ; intervalle entre les examens exact dans Girder.
4. Envoi suivant, autre patient ⇒ alerte dans la fenêtre de revue. Annulé : rien n'est
   envoyé ni tracé. Validé : envoi effectué, signature du record inchangée, alerte dans les
   deux journaux et sur le champ, y compris après actualisation depuis Girder.
5. Deux NIP dans un même lot ⇒ lot refusé.
6. Deux premiers envois simultanés avec des NIP différents ⇒ un seul passe : pour l'autre,
   la discordance apparaît après sa validation.
7. Limite de débit atteinte, ou Girder arrêté ⇒ refus, rien n'est stocké ni envoyé.
8. Contenu du record supprimé ⇒ signature effacée ; un autre NIP peut alors être envoyé.
9. Contrôle décoché ⇒ un autre patient est accepté, rien n'est stocké ; dates décalées.
10. Revue annulée, par le bouton, Entrée ou Échap ⇒ rien n'est stocké ni envoyé, le champ
    reste vide et l'identité quitte la page.
11. Un même patient sur deux records ⇒ même décalage, UID différents.
12. Fichier dont le nom patient et la description de série contiennent du HTML actif ⇒
    affiché comme texte, jamais exécuté.
13. Dossier d'un autre record, ou dossier du record lui-même, demandé à la suppression
    ou à l'actualisation ⇒ refus ; DICOM contenant un PDF, ou Secondary Capture ⇒ mis de
    côté, le reste passe.
14. Montée de version réelle depuis une version publiée (`tests/e2e/run.sh upgrade <tag>`) :
    voir §9.

## 9. Compatibilité

Ce schéma remplace l'ancrage des dates sur la date de naissance. Les fichiers déjà envoyés
restent sur leur ancien calendrier ; `DeidentificationMethod` permet de distinguer les
générations (sans version : ancrage sur la naissance ; `v2.0.0` et suivantes : décalage par
pseudonyme).

La montée de version a été testée pour de vrai, depuis v1.2.0 et v1.5.0, sur REDCap 15.9.3 :
envoi avec la version publiée, mise à jour par la route du Control Center, puis usage des
données.

| Point | Constat |
|---|---|
| Fichiers dans Girder | intacts (empreinte SHA-256 identique), aucun chemin ne change ; les nouveaux envois vont sous `{event_id} ({event_name})`, et le comptage et la suppression reconnaissent les deux organisations |
| Champs des records | anciens formats toujours affichés, actualisés depuis Girder, supprimables ; un envoi en échec peut être repris |
| Clé `k` | créée à la mise à jour : `redcap_module_system_enable` est appelé aussi dans ce cas depuis REDCap 12.0.4 ; la création à la première évaluation couvre les versions antérieures |
| Réglages de projet | conservés : REDCap n'applique les valeurs par défaut qu'à l'activation sur un projet. Le contrôle est actif par le défaut du code, sans enregistrer les réglages |
| Page ouverte pendant la mise à jour | **refusée** : elle ferait passer des fichiers désidentifiés à l'ancienne sans ticket, puisqu'elle n'en déclare aucun comme désidentifié |
| PHP sans ristretto255 | la version est refusée (`compatibility.php-version-min`) et REDCap désactive alors le module sur toute l'instance, jusqu'à ce que l'ancienne version soit réactivée : aucune donnée perdue, mais PHP est à vérifier avant la mise à jour. Si ristretto255 manque malgré un PHP récent, l'évaluation échoue avec un message explicite, sans rien envoyer |

Trois écarts restent, inhérents au changement de méthode :

- **Dates** : l'intervalle entre un fichier d'avant et un fichier d'après n'a pas de sens
  (plus de 25 000 jours mesurés pour 31 réels). Suivre un patient sur les deux suppose de renvoyer
  les examens anciens depuis la source.
- **UID** : une étude renvoyée apparaît comme une nouvelle étude, à côté de l'ancienne.
- **Signature** : elle ne peut pas être calculée pour les envois existants, puisque le NIP
  n'est plus dans les fichiers. Le premier envoi après la mise à jour la pose sans pouvoir
  la comparer, et la fenêtre de revue le dit.
