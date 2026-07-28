pub(crate) const PHI_BANNER: &str = r#"
         ⢀⡴⠂
   ⢀⣠⣤⠶⠒⣠⠎⣀⡀    █████╗  ██████╗ ███████╗███╗   ██╗████████╗
  ⣰⣿⠏ ⣠⡾⠃ ⣸⣿   ██╔══██╗██╔════╝ ██╔════╝████╗  ██║╚══██╔══╝
  ⣿⣏⢠⣾⣿⡁⢀⣰⣿⠋   ███████║██║  ███╗█████╗  ██╔██╗ ██║   ██║
   ⠉⣰⡿⠫⠶⠛⠋     ██╔══██║██║   ██║██╔══╝  ██║╚██╗██║   ██║
  ⢠⠞⠁          ██║  ██║╚██████╔╝███████╗██║ ╚████║   ██║
               ╚═╝  ╚═╝ ╚═════╝ ╚══════╝╚═╝  ╚═══╝   ╚═╝

"#;

pub(crate) fn startup_banner() -> &'static str {
    PHI_BANNER.trim_matches('\n')
}

#[cfg(test)]
mod tests {
    use super::startup_banner;

    #[test]
    fn startup_banner_uses_braille_cells() {
        assert!(
            startup_banner()
                .chars()
                .any(|cell| ('\u{2800}'..='\u{28ff}').contains(&cell))
        );
    }

    #[test]
    fn startup_banner_stays_compact() {
        let banner = startup_banner();

        assert!(banner.lines().count() <= 7);
        assert!(banner.lines().all(|line| line.chars().count() <= 60));
        assert!(!banner.lines().next().unwrap_or_default().contains('█'));
        assert!(banner.lines().last().unwrap_or_default().contains("╚═╝"));
    }
}
