//! Manual smoke test: scan the real machine and print the chain topology.
//! Run: cargo run --example chain_scan [warehouse_root] [projects_root]

use std::path::PathBuf;

fn main() {
    let home = dirs::home_dir().expect("home");
    let args: Vec<String> = std::env::args().collect();
    let warehouse = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("Projects/xw-skills"));
    let projects = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("Projects"));

    // The app draws its project inventory from Patchbay's registered-project
    // records. This standalone smoke test has no database, so it approximates
    // the inventory by enumerating the projects root's child directories.
    let project_paths: Vec<PathBuf> = std::fs::read_dir(&projects)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();

    let adapters = app_lib::core::tool_adapters::default_tool_adapters();
    let managed = app_lib::core::central_repo::skills_dir();
    let topo = app_lib::core::chain::build_topology(
        &[warehouse],
        &managed,
        &projects,
        &project_paths,
        &adapters,
    );

    println!("== roots ==");
    for root in &topo.warehouse_roots {
        let err = root
            .error
            .as_deref()
            .map(|e| format!("  ({e})"))
            .unwrap_or_default();
        println!(
            "  {:10} repos={:3} {}{}",
            root.status, root.repo_count, root.root, err
        );
    }
    println!("== guard ==");
    for g in &topo.guard {
        // Absent surfaces are the compliant common case; skip them to keep the
        // smoke output focused on directories that actually exist.
        if g.state == "absent" {
            continue;
        }
        println!("  {:16} {:10} {}", g.agent, g.state, g.path);
        for v in &g.violations {
            println!(
                "      violation: {} -> {} (link={})",
                v.skill, v.final_target, v.is_link
            );
        }
    }
    println!("== repos ({}) ==", topo.repos.len());
    for r in &topo.repos {
        let h = &r.health;
        let origin = r.origin.as_ref().map(|o| o.url.as_str()).unwrap_or("-");
        let upstream = r.upstream.as_ref().map(|u| u.url.as_str()).unwrap_or("-");
        let refs: Vec<&str> = r.referenced_by.iter().map(|p| p.name.as_str()).collect();
        println!(
            "  {:24} root={:20} {:11} dirty={:5} ahead={:2} behind={:2} skills={:3} referenced_by={:?}",
            r.name,
            r.root,
            h.state,
            h.dirty,
            h.ahead,
            h.behind,
            r.skills.len(),
            refs
        );
        println!("      origin={origin} upstream={upstream}");
    }
    println!("== projects ({}) ==", topo.projects.len());
    for p in &topo.projects {
        let agg = p
            .agents_dir
            .as_ref()
            .map(|a| format!("{} entries", a.entries.len()))
            .unwrap_or_else(|| "none".into());
        println!("  {} (.agents/skills: {})", p.name, agg);
        if let Some(a) = &p.agents_dir {
            let mut counts = std::collections::BTreeMap::new();
            for e in &a.entries {
                *counts.entry(e.status.clone()).or_insert(0) += 1;
            }
            println!("      agg statuses: {counts:?}");
        }
        for s in &p.surfaces {
            if s.kind == "absent" {
                continue;
            }
            let mut counts = std::collections::BTreeMap::new();
            for e in &s.entries {
                *counts.entry(e.status.clone()).or_insert(0) += 1;
            }
            println!(
                "      {:9} kind={:9} dir_link_ok={:5} entries={:?}",
                s.agent, s.kind, s.dir_link_ok, counts
            );
        }
    }
}
