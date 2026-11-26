//! Réglages d'une conversion, partagés par la fenêtre, la ligne de commande et le serveur MCP.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Bas-relief : la luminosité devient la hauteur, sur une plaque.
    Relief,
    /// Extrusion : la silhouette de l'objet devient une pièce épaisse.
    Extrude,
    /// Gonflage : la silhouette devient une forme arrondie, comme un coussin.
    Inflate,
    /// Multi-vues : plusieurs côtés (face, profil, dos, dessus) reconstruits en un objet complet.
    Multiview,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Multiview, Mode::Inflate, Mode::Extrude, Mode::Relief];
    pub fn id(self) -> &'static str {
        match self {
            Mode::Relief => "relief",
            Mode::Extrude => "extrude",
            Mode::Inflate => "inflate",
            Mode::Multiview => "multiview",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Relief => "Bas-relief",
            Mode::Extrude => "Extrusion",
            Mode::Inflate => "Gonflage",
            Mode::Multiview => "Multi-vues (personne, véhicule, bâtiment)",
        }
    }
    pub fn help(self) -> &'static str {
        match self {
            Mode::Multiview => "Une planche avec plusieurs côtés (face, profil, dos…) ou une image par côté : l'objet est reconstruit en volume complet, texturé avec les photos de chaque côté.",
            Mode::Relief => "La luminosité devient la hauteur (clair = haut). Idéal pour photos, cartes de hauteur, textures, terrains.",
            Mode::Extrude => "Le fond est retiré et la silhouette est extrudée en pièce épaisse. Idéal pour logos, icônes, sprites.",
            Mode::Inflate => "Le fond est retiré et la silhouette est gonflée en volume arrondi. Idéal pour personnages, objets, dessins.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// glTF binaire, texture incluse (recommandé pour Blender).
    Glb,
    /// OBJ + MTL + texture PNG.
    Obj,
    /// STL binaire (impression 3D, sans couleur).
    Stl,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Glb, Format::Obj, Format::Stl];
    pub fn id(self) -> &'static str {
        match self {
            Format::Glb => "glb",
            Format::Obj => "obj",
            Format::Stl => "stl",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Format::Glb => "GLB (glTF, avec texture)",
            Format::Obj => "OBJ + MTL + texture",
            Format::Stl => "STL (impression 3D)",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Background {
    /// Transparence si l'image en a, sinon couleur du bord (relief : garde tout).
    Auto,
    /// Utilise la transparence de l'image.
    Alpha,
    /// Retire la couleur du bord de l'image.
    Color,
    /// Garde toute l'image.
    None,
}

impl Background {
    pub const ALL: [Background; 4] = [Background::Auto, Background::Alpha, Background::Color, Background::None];
    pub fn label(self) -> &'static str {
        match self {
            Background::Auto => "Automatique",
            Background::Alpha => "Transparence",
            Background::Color => "Couleur du bord",
            Background::None => "Aucun (garder tout)",
        }
    }
}

/// Multi-vues : le genre d'objet, pour arrondir (ou non) la forme entre les vues.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    /// Personnes, animaux, personnages : sections arrondies.
    Organic,
    /// Une personne : sections arrondies, et si le dos n'est pas fourni, un dos est créé
    /// (cheveux à l'arrière de la tête, pas de visage, vêtements sans leurs détails de face).
    Person,
    /// Voitures, motos, objets : sections arrondies mais carrées.
    Vehicle,
    /// Bâtiments, meubles : formes droites.
    Building,
}

impl Shape {
    pub const ALL: [Shape; 4] = [Shape::Person, Shape::Organic, Shape::Vehicle, Shape::Building];
    /// Forme arrondie (personne, animal, personnage).
    pub fn is_organic(self) -> bool {
        matches!(self, Shape::Organic | Shape::Person)
    }
    pub fn label(self) -> &'static str {
        match self {
            Shape::Person => "Personne",
            Shape::Organic => "Animal / personnage arrondi",
            Shape::Vehicle => "Voiture / moto / objet",
            Shape::Building => "Bâtiment / meuble",
        }
    }
}

/// Moteur de reconstruction multi-vues.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// Sculpture des silhouettes, sur l'ordinateur : gratuit, hors ligne, en une seconde.
    Local,
    /// IA en ligne Tripo 3D : invente les parties cachées, bien plus détaillé, payant.
    Tripo,
}

impl Engine {
    pub fn label(self) -> &'static str {
        match self {
            Engine::Local => "Local (gratuit, hors ligne)",
            Engine::Tripo => "IA Tripo 3D (en ligne, payant)",
        }
    }
}

/// Côté représenté par une vue (mode multi-vues).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ViewAngle {
    /// Deviné d'après la forme.
    Auto,
    /// Face.
    Front,
    /// Profil, l'objet regarde vers la droite de l'image.
    SideRight,
    /// Dos.
    Back,
    /// Profil, l'objet regarde vers la gauche de l'image.
    SideLeft,
    /// Vue de dessus (l'avant de l'objet en bas de l'image).
    Top,
    /// Vue non utilisée (trois-quarts, détail…).
    Ignore,
}

impl ViewAngle {
    pub const ALL: [ViewAngle; 7] =
        [ViewAngle::Auto, ViewAngle::Front, ViewAngle::SideRight, ViewAngle::Back, ViewAngle::SideLeft, ViewAngle::Top, ViewAngle::Ignore];
    pub fn id(self) -> &'static str {
        match self {
            ViewAngle::Auto => "auto",
            ViewAngle::Front => "front",
            ViewAngle::SideRight => "side_right",
            ViewAngle::Back => "back",
            ViewAngle::SideLeft => "side_left",
            ViewAngle::Top => "top",
            ViewAngle::Ignore => "ignore",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            ViewAngle::Auto => "Automatique",
            ViewAngle::Front => "Face",
            ViewAngle::SideRight => "Profil (regarde à droite)",
            ViewAngle::Back => "Dos",
            ViewAngle::SideLeft => "Profil (regarde à gauche)",
            ViewAngle::Top => "Dessus",
            ViewAngle::Ignore => "Ignorer",
        }
    }
    pub fn parse(s: &str) -> Option<ViewAngle> {
        let s = s.trim().to_ascii_lowercase();
        ViewAngle::ALL.into_iter().find(|a| a.id() == s).or(match s.as_str() {
            "face" | "avant" => Some(ViewAngle::Front),
            "dos" | "arriere" | "rear" => Some(ViewAngle::Back),
            "side" | "profil" | "right" => Some(ViewAngle::SideRight),
            "left" => Some(ViewAngle::SideLeft),
            "dessus" => Some(ViewAngle::Top),
            "ignorer" | "skip" => Some(ViewAngle::Ignore),
            _ => None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub mode: Mode,
    /// Multi-vues : genre d'objet.
    pub shape: Shape,
    /// Multi-vues : moteur local (gratuit, hors ligne) ou IA en ligne Tripo (payant, détaillé).
    pub engine: Engine,
    /// IA : "h3.1" (détail maximal) ou "p2" (maillage léger en quadrilatères, pour les jeux).
    pub ai_model: String,
    /// IA : nombre de faces visé (0 = laissé à l'IA).
    pub ai_faces: u32,
    /// IA : texture détaillée (plus chère) au lieu de standard.
    pub ai_detailed_texture: bool,
    /// IA : matériaux PBR (métal, rugosité, relief).
    pub ai_pbr: bool,
    pub format: Format,
    /// Nombre de cellules sur le plus grand côté (détail du maillage), 8 à 2048.
    pub resolution: u32,
    /// Épaisseur / hauteur en unités Blender (mètres).
    pub depth: f32,
    /// Taille du plus grand côté en unités Blender (mètres).
    pub size: f32,
    /// Épaisseur de la plaque sous le relief.
    pub base: f32,
    /// Inverse la luminosité (sombre = haut).
    pub invert: bool,
    pub background: Background,
    /// Écart de couleur pour retirer le fond (0-441).
    pub threshold: f32,
    /// Passes de lissage de la luminosité.
    pub smooth: u32,
    /// Gonflage : part du détail de luminosité ajouté à la forme (0-1).
    pub detail: f32,
    /// Gonflage : épaisseur du dos par rapport à l'avant (0 = dos plat, 1 = symétrique).
    pub back_scale: f32,
    /// Taille maximale de la texture exportée (pixels).
    pub texture_max: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mode: Mode::Inflate,
            shape: Shape::Person,
            engine: Engine::Local,
            ai_model: "h3.1".into(),
            ai_faces: 0,
            ai_detailed_texture: false,
            ai_pbr: true,
            format: Format::Glb,
            resolution: 256,
            depth: 0.3,
            size: 2.0,
            base: 0.04,
            invert: false,
            background: Background::Auto,
            threshold: 40.0,
            smooth: 1,
            detail: 0.15,
            back_scale: 1.0,
            texture_max: 2048,
        }
    }
}
