//! Writes `crates/pathon-pbr/src/ltc_table.rs`: `cargo run -p pathon-ltc-fit --release [-- output.rs]`.

use pathon_ltc_fit::{SIZE, fit_table, table_source};

const DEFAULT_OUTPUT: &str = "crates/pathon-pbr/src/ltc_table.rs";

#[expect(clippy::print_stdout, reason = "a command line tool reports its result")]
fn main() -> std::io::Result<()> {
    let output = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_OUTPUT.to_owned());
    let entries = fit_table();
    let mean = entries.iter().map(|entry| entry.total_variation).sum::<f64>() / entries.len() as f64;
    let worst = entries
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_variation.total_cmp(&b.1.total_variation))
        .expect("a table has entries");
    println!(
        "{SIZE}x{SIZE} nodes; total variation of lobe vs fit: mean {mean:.4}, worst {:.4} at roughness node {}, view node {}",
        worst.1.total_variation,
        worst.0 % SIZE,
        worst.0 / SIZE
    );
    println!("total variation, rows: view node (cos), columns: roughness node 0, 4, 8, ... 28, 31");
    for view in (0..SIZE).step_by(4).chain([SIZE - 1]) {
        let cells: Vec<String> = (0..SIZE)
            .step_by(4)
            .chain([SIZE - 1])
            .map(|roughness| format!("{:.3}", entries[view * SIZE + roughness].total_variation))
            .collect();
        println!("{:>2} ({:.3}): {}", view, pathon_ltc_fit::cos_view_of(view), cells.join(" "));
    }
    std::fs::write(&output, table_source(&entries))?;
    println!("wrote {output}");
    Ok(())
}
