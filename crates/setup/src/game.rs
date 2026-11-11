//! Finding the game: every install of Euro Truck Simulator 2 on the PC (Steam's, in any of its libraries, and
//! others Windows lists as installed), each with its version (its executable's; on macOS its pack set's) and its
//! DLCs.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::platform::{EXECUTABLE, PLUGINS_DIR, PROCESS};
use crate::{places, platform, vdf};

/// The game's app id on Steam (its manifest, its Workshop folder, its Steam Cloud folder).
pub const STEAM_APP: &str = "227300";
/// The game versions WestPlay has been tried with. Others may well work (the plugin finds what it needs in
/// the game by its shape), but haven't been tried.
#[cfg(not(target_os = "macos"))]
pub const TESTED_VERSIONS: &str = "1.61.*";
/// The game versions WestPlay has been tried with: the Mac game is still 1.57 (docs/MACOS_PORT.md, Phase 0), and
/// WestPlay's port was made and tried with it.
#[cfg(target_os = "macos")]
pub const TESTED_VERSIONS: &str = "1.57.*";

/// An install of the game.
#[derive(Clone, Debug, PartialEq)]
pub struct Game {
    /// Its folder (with `base.scs`).
    pub dir: PathBuf,
    pub found_by: FoundBy,
    /// Its version, from its executable (on macOS, its `version.scs`; `None`: it couldn't be read).
    pub version: Option<Version>,
    /// Its DLCs' archives, by name (`dlc_balkan_e`…), in order.
    pub dlcs: Vec<String>,
}

/// How an install was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoundBy {
    /// In one of Steam's libraries.
    Steam,
    /// In Windows' list of installed programs (another store's, or SCS's own installer).
    Installed,
    /// The player chose its folder.
    Chosen,
}

/// The game's version: `1.61.1.9`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub [u16; 4]);

impl Version {
    /// Whether a version pattern of the game's (`1.61.*`, `1.61.1.9`) takes in this version.
    pub fn matches(&self, pattern: &str) -> bool {
        let parts: Vec<&str> = pattern.trim().split('.').collect();
        parts.iter().enumerate().all(|(index, part)| {
            *part == "*" || (index < 4 && part.parse::<u16>().is_ok_and(|number| number == self.0[index]))
        }) && !parts.is_empty()
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c, d] = self.0;
        write!(f, "{a}.{b}.{c}.{d}")
    }
}

impl Game {
    /// The game in `dir`, if it's an install of it: its archives and its executable are there.
    pub fn at(dir: &Path, found_by: FoundBy) -> Option<Self> {
        if !dir.join("base.scs").is_file() || !dir.join(EXECUTABLE).is_file() {
            return None;
        }
        let mut dlcs: Vec<String> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_owned();
                let dlc = name.strip_prefix("dlc_").is_some() && name.ends_with(".scs");
                dlc.then(|| name.trim_end_matches(".scs").to_owned())
            })
            .collect();
        dlcs.sort();
        Some(Self { dir: dir.to_path_buf(), found_by, version: platform::game_version(dir), dlcs })
    }

    /// The game in `dir`, found as [`find`] finds it (Steam's, or listed as installed) or else as chosen.
    pub fn recognise(dir: &Path) -> Option<Self> {
        find().into_iter().find(|game| same_folder(&game.dir, dir)).or_else(|| Self::at(dir, FoundBy::Chosen))
    }

    /// Whether WestPlay has been tried with this version of the game ([`TESTED_VERSIONS`]).
    pub fn tested(&self) -> bool {
        self.version.is_some_and(|version| version.matches(TESTED_VERSIONS))
    }

    /// Where Steam keeps the game's Workshop mods, for an install in a Steam library (however it was found:
    /// a folder chosen there too).
    pub fn workshop(&self) -> Option<PathBuf> {
        // `<library>/steamapps/common/<game>` → `<library>/steamapps/workshop/content/227300`.
        let common = self.dir.parent()?;
        let steamapps = common.parent()?;
        let named = |path: &Path, name: &str| path.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name));
        if !named(common, "common") || !named(steamapps, "steamapps") {
            return None;
        }
        let workshop = steamapps.join("workshop").join("content").join(STEAM_APP);
        workshop.is_dir().then_some(workshop)
    }

    /// Where the plugin goes.
    pub fn plugins_dir(&self) -> PathBuf {
        PLUGINS_DIR.iter().fold(self.dir.clone(), |dir, part| dir.join(part))
    }
}

/// Every install of the game found on the PC: Steam's first (in each of its libraries), then others Windows
/// lists as installed. Each folder once.
pub fn find() -> Vec<Game> {
    let mut found: Vec<Game> = Vec::new();
    let mut add = |game: Option<Game>| {
        if let Some(game) = game
            && !found.iter().any(|known| same_folder(&known.dir, &game.dir))
        {
            found.push(game);
        }
    };
    if let Some(steam) = places::steam() {
        for library in places::steam_libraries(&steam) {
            let steamapps = library.join("steamapps");
            let manifest = std::fs::read_to_string(steamapps.join(format!("appmanifest_{STEAM_APP}.acf"))).ok();
            if let Some(folder) = manifest.as_deref().and_then(steam_install_dir) {
                add(Game::at(&steamapps.join("common").join(folder), FoundBy::Steam));
            }
        }
    }
    for (name, location) in places::installed_programs() {
        if is_the_game(&name) {
            // Steam lists its games too: those are found already.
            add(Game::at(&location, FoundBy::Installed));
        }
    }
    found
}

/// Whether the game is running (any install of it).
pub fn running() -> bool {
    platform::process_running(PROCESS)
}

/// The game's folder name in its Steam library, from its `appmanifest_227300.acf`.
pub fn steam_install_dir(manifest: &str) -> Option<String> {
    let file = vdf::parse(manifest)?;
    let state = file.get("AppState")?;
    (state.get("appid")?.text()? == STEAM_APP).then_some(())?;
    Some(state.get("installdir")?.text()?.to_owned())
}

/// Whether a program Windows lists is the game (not its demo, nor American Truck Simulator).
pub fn is_the_game(name: &str) -> bool {
    let name = name.to_lowercase();
    name.contains("euro truck simulator 2") && !name.contains("demo")
}

fn same_folder(a: &Path, b: &Path) -> bool {
    let key = |path: &Path| {
        std::fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .to_lowercase()
            .trim_end_matches(['\\', '/'])
            .to_owned()
    };
    key(a) == key(b)
}

/// The version a pack set's `version.sii` (in the game's `version.scs`) gives: `version: "1.57.2.7"`.
pub fn pack_version(text: &str) -> Option<Version> {
    let units = westplay_mapdata::sii::parse(text);
    let version = units.iter().find_map(|unit| unit.get("version"))?;
    let mut parts = [0u16; 4];
    let mut count = 0;
    for (slot, part) in parts.iter_mut().zip(version.trim_matches('"').split('.')) {
        *slot = part.parse().ok()?;
        count += 1;
    }
    (count >= 2).then_some(Version(parts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_workshop_of_the_steam_library_the_game_is_in() {
        let root = std::env::temp_dir().join(format!("westplay-workshop-{}", std::process::id()));
        let library = root.join("SteamLibrary").join("steamapps");
        let workshop = library.join("workshop").join("content").join(STEAM_APP);
        std::fs::create_dir_all(&workshop).unwrap();
        let game = |dir: PathBuf| Game { dir, found_by: FoundBy::Chosen, version: None, dlcs: vec![] };
        // Chosen there by hand, it's still that library's game.
        assert_eq!(game(library.join("common").join("Euro Truck Simulator 2")).workshop(), Some(workshop));
        // Anywhere else, there's none.
        assert_eq!(game(root.join("Games").join("Euro Truck Simulator 2")).workshop(), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tells_a_tested_version() {
        let game = |version| Game { dir: PathBuf::new(), found_by: FoundBy::Steam, version, dlcs: vec![] };
        // Each system's own game: Windows' 1.61, the Mac's 1.57.
        let (tested, other) =
            if cfg!(target_os = "macos") { ([1, 57, 2, 7], [1, 61, 1, 9]) } else { ([1, 61, 1, 9], [1, 57, 2, 7]) };
        assert!(game(Some(Version(tested))).tested());
        assert!(!game(Some(Version(other))).tested());
        assert!(!game(Some(Version([1, 62, 0, 1]))).tested());
        assert!(!game(None).tested());
    }

    #[test]
    fn reads_the_folder_steam_installed_the_game_in() {
        let manifest = "\"AppState\"\n{\n\t\"appid\"\t\t\"227300\"\n\t\"installdir\"\t\t\"Euro Truck Simulator 2\"\n}";
        assert_eq!(steam_install_dir(manifest).as_deref(), Some("Euro Truck Simulator 2"));
        // Another game's manifest.
        assert_eq!(steam_install_dir("\"AppState\" { \"appid\" \"270880\" \"installdir\" \"American\" }"), None);
    }

    #[test]
    fn knows_the_game_in_windows_list_of_programs() {
        assert!(is_the_game("Euro Truck Simulator 2"));
        assert!(!is_the_game("Euro Truck Simulator 2 Demo"));
        assert!(!is_the_game("American Truck Simulator"));
    }

    #[test]
    fn matches_the_games_version_patterns() {
        let version = Version([1, 61, 1, 9]);
        assert_eq!(version.to_string(), "1.61.1.9");
        assert!(version.matches("1.61.*") && version.matches("1.61.1.9") && version.matches("1.*"));
        assert!(!version.matches("1.60.*") && !version.matches("1.6.*") && !version.matches(""));
    }

    #[test]
    fn reads_the_version_of_a_pack_set() {
        let text = "SiiNunit\n{\nfs_pack_set : _nameless.220.d670 {\n application: eut2\n version: \"1.57.2.7\"\n platforms: 1\n}\n}\n";
        assert_eq!(pack_version(text), Some(Version([1, 57, 2, 7])));
        assert_eq!(pack_version("SiiNunit { fs_pack_set : x { application: eut2 } }"), None);
        assert_eq!(pack_version("SiiNunit { fs_pack_set : x { version: \"one\" } }"), None);
    }

    /// The real thing, on this PC: prints what's found. `cargo test -p westplay-setup -- --ignored --nocapture`
    #[test]
    #[ignore = "looks at the PC's installs"]
    fn finds_the_game_on_this_pc() {
        for game in find() {
            println!(
                "{} ({:?}): version {:?}, {} DLCs, workshop {:?}",
                game.dir.display(),
                game.found_by,
                game.version.map(|v| v.to_string()),
                game.dlcs.len(),
                game.workshop()
            );
        }
    }
}
