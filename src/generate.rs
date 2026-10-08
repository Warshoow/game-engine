//! Génération d'un bloc en jeu (#18) : touche I, le joueur décrit le bloc,
//! un [`Generator`] rend une entrée en RON, `voxel_core::generate::accept`
//! la valide, puis elle entre au registre par le même chemin que la touche
//! R (`content::apply_content`).
//!
//! L'appel dure plusieurs secondes : il tourne dans un thread, le jeu
//! continue et lit le résultat quand il arrive.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{Arc, OnceLock};

use bevy::prelude::*;

use voxel_core::generate;

use crate::content::Incoming;
use crate::ui::{self, TextField};
use crate::{assets_dir, GameWorld};

/// Ce qui produit le contenu : un texte entre, un texte sort. Bloquant.
/// Aujourd'hui la CLI `claude` ; demain une clé API du joueur (BYOK) — une
/// implémentation de plus, rien d'autre ne change.
pub trait Generator: Send + Sync {
    fn generate(&self, prompt: &str) -> Result<String, String>;
}

/// La CLI Claude Code (`claude -p`), qui doit être dans le PATH.
/// `--tools ""` : aucun outil, le modèle ne peut que répondre du texte ;
/// `--safe-mode` : ni hooks, ni CLAUDE.md, ni plugins de la machine.
pub struct ClaudeCli;

const SYSTEM: &str = "Tu génères du contenu pour un jeu voxel. Tu réponds uniquement par la donnée demandée.";

impl Generator for ClaudeCli {
    fn generate(&self, prompt: &str) -> Result<String, String> {
        let spawn = |program: &std::path::Path| {
            Command::new(program)
                .args(["-p", "--safe-mode", "--tools", "", "--no-session-persistence", "--system-prompt", SYSTEM])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
        };
        // Le PATH du jeu est celui du programme qui l'a lancé : sous Windows,
        // l'Explorateur garde souvent l'ancien. Repli : l'emplacement où
        // l'installeur de Claude Code le met (`~/.local/bin`).
        let installed = std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(|home| std::path::Path::new(&home).join(".local/bin/claude"));
        let mut child = spawn("claude".as_ref())
            .or_else(|e| installed.as_deref().map_or(Err(e), spawn))
            .map_err(|e| format!("claude introuvable, ni dans le PATH ni dans ~/.local/bin ({e})"))?;
        child
            .stdin
            .take()
            .expect("stdin demandé")
            .write_all(prompt.as_bytes())
            .map_err(|e| e.to_string())?;
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(format!("claude a échoué : {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// Le générateur utilisé, et la génération en cours.
#[derive(Resource)]
pub struct Generation {
    pub generator: Arc<dyn Generator>,
    /// Rempli par le thread quand la réponse arrive.
    pending: Option<Arc<OnceLock<Result<String, String>>>>,
    /// Affiché dans la fenêtre.
    status: String,
}

impl Default for Generation {
    fn default() -> Self {
        Self { generator: Arc::new(ClaudeCli), pending: None, status: String::new() }
    }
}

/// La fenêtre de génération.
#[derive(Component)]
pub struct GenerateWindow;

/// Le texte d'état dans la fenêtre.
#[derive(Component)]
pub struct StatusText;

/// Touche I : ouvre ou ferme la fenêtre de génération.
pub fn toggle_generate_window(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    opened: Query<Entity, With<ui::GameWindow>>,
    window: Query<Entity, With<GenerateWindow>>,
    generation: Res<Generation>,
) {
    if !keys.just_pressed(KeyCode::KeyI) {
        return;
    }
    if let Ok(window) = window.single() {
        commands.entity(window).despawn();
        return;
    }
    let window = ui::open(&mut commands, &opened, "Générer un bloc");
    commands.entity(window).insert(GenerateWindow).with_children(|w| {
        w.spawn(Text::new("Décris le bloc, puis Entrée :"));
        w.spawn((TextField::default(), Text::default(), Node { min_width: Val::Px(500.0), ..default() }));
        w.spawn((StatusText, Text::new(generation.status.clone())));
    });
}

/// Entrée dans le champ : lance la génération dans un thread.
pub fn start_generation(
    keys: Res<ButtonInput<KeyCode>>,
    game: Res<GameWorld>,
    mut generation: ResMut<Generation>,
    field: Query<&TextField>,
) {
    let Ok(field) = field.single() else { return };
    if !keys.just_pressed(KeyCode::Enter) || field.0.trim().is_empty() || generation.pending.is_some() {
        return;
    }
    let prompt = generate::prompt(&game.world.registry, field.0.trim());
    let slot = Arc::new(OnceLock::new());
    let (generator, result) = (generation.generator.clone(), slot.clone());
    std::thread::spawn(move || {
        let _ = result.set(generator.generate(&prompt));
    });
    generation.pending = Some(slot);
    generation.status = "génération…".into();
}

/// Réponse arrivée : validée, puis passée à `apply_content`. Un refus
/// s'affiche, le joueur relance.
pub fn finish_generation(game: Res<GameWorld>, mut generation: ResMut<Generation>, mut incoming: ResMut<Incoming>) {
    let Some(result) = generation.pending.as_ref().and_then(|slot| slot.get()).cloned() else { return };
    generation.pending = None;
    let accepted = result.and_then(|answer| {
        let entry = generate::accept(&game.world.registry, &answer).map_err(|e| {
            warn!("réponse refusée : {e}\n{answer}");
            format!("refusé : {e}")
        })?;
        let identifier = entry.identifier.clone();
        let mut registry = game.world.registry.clone();
        registry.register(entry).map_err(|e| format!("{e:?}"))?;
        registry.load_textures(&assets_dir().join("textures")).map_err(|e| e.to_string())?;
        Ok((identifier, registry))
    });
    generation.status = match accepted {
        Ok((identifier, registry)) => {
            incoming.0 = Some(registry);
            format!("{identifier} ajouté au registre (G pour l'avoir en main)")
        }
        Err(e) => e,
    };
}

/// Affiche l'état dans la fenêtre ouverte.
pub fn show_status(generation: Res<Generation>, mut text: Query<&mut Text, With<StatusText>>) {
    if generation.is_changed() {
        for mut text in &mut text {
            text.0 = generation.status.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Appel réel à `claude` (lent, demande la CLI) :
    /// `cargo test -- --ignored claude`.
    #[test]
    #[ignore]
    fn claude_answer_is_accepted() {
        let registry = crate::content::load(voxel_core::registry::Registry::new()).unwrap();
        let prompt = generate::prompt(&registry, "un bloc de lave qui se refroidit en pierre quand on clique dessus");
        let answer = ClaudeCli.generate(&prompt).unwrap();
        generate::accept(&registry, &answer).unwrap_or_else(|e| panic!("{e}\n{answer}"));
    }
}
