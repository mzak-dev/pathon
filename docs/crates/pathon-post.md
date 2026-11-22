# pathon-post

Post-processing (M4-09): ekspozycja, bloom i tonemapping SDR jako jedna paczka. Zamienia liniowe `hdr` w obraz celu: `hdr × exposure` (+ bloom) → krzywa tonemappingu → transfer wyjściowy → slot `output`. Decyzje i uzasadnienie wyboru operatorów: [ADR-0038](../adr/0038-post-processing-ekspozycja-bloom-tonemapping.md). Zależy tylko od `pathon-api`.

## Użycie

```rust,ignore
use pathon_post::{PostPack, PostSettings, Tonemapper};

let mut settings = PostSettings::default();
settings.tonemap.operator = Tonemapper::PbrNeutral;
let mut renderer = Renderer::builder()
    .with_pack(PostPack::with_settings(settings))   // paczka pbr / własne passy piszą `Slot::HDR`
    .build()?;

// ustawienia zmienia się między klatkami
renderer.shared_mut().get_mut::<PostSettings>().expect("post pack").bloom.intensity = 0.3;
```

`PostPack::new()` używa domyślnych ustawień. `with_shared(PostSettings…)` wołane *po* `with_pack` nadpisuje ustawienia paczki; użyj jednego z dwóch sposobów. Przykład: `cargo run -p pathon-examples --bin post_chain -- aces wynik.png` (syntetyczna klatka HDR przez cały łańcuch, wynik w PNG).

## Kontrakt slotów

| Slot | Kierunek | Format | Znaczenie |
| --- | --- | --- | --- |
| `hdr` | czyta | `Rgba16Float`, render | liniowa luminancja sceny, bez ekspozycji; NaN i wartości ujemne czytane jako 0, `+Inf` jako 65504 |
| `exposure` | **pisze** (`ExposureFeature`), czyta bloom i tonemapping | `R32Float` 1×1 | mnożnik: `hdr × exposure` to jasność wyświetlana (0.18 = środkowa szarość) |
| `post.bloom` (`pathon_post::BLOOM`) | **pisze** (`BloomFeature`), czyta tonemapping | `Rgba16Float`, połowa rozmiaru renderu | rozmyta jasna część klatki w jednostkach po ekspozycji; nie ma go, gdy bloom wyłączony |
| `output` | **pisze** (`TonemapFeature`) | format celu | obraz końcowy; pass czyści cały cel |

Bez slotu `hdr` żaden pass nie rusza `output`. Bez slotu `exposure` tonemapping jest pomijany (Q0003): własna paczka ekspozycji może zastąpić `ExposureFeature`, jeśli zapisze `Slot::EXPOSURE`.

Kolejność w grafie: `ExposureFeature` i `BloomFeature` w `Stage::PostProcess` (bloom po ekspozycji), `TonemapFeature` w `Stage::Tonemap` z rolą `Role::TONEMAP` (druga paczka z tą rolą dostaje F0102).

## Ustawienia (`PostSettings`)

Wartości poza zakresem są naprawiane (przycięcie, NaN → domyślna) i zgłaszane raz jako Q0001.

| Pole | Domyślnie | Zakres | Znaczenie |
| --- | --- | --- | --- |
| `exposure.mode` | `Auto(AutoExposure::default())` | — | `Auto` albo `Manual { ev }` |
| `exposure.compensation_ev` | 0 | −16…16 | stopnie dodane do trybu; dodatnie rozjaśnia |
| `Manual { ev }` | — | −32…32 | mnożnik `2^-ev` (`ev = 0` zostawia `hdr` bez zmian) |
| `auto.min_ev` / `max_ev` | −9 / 13 | −24…24, `max ≥ min + 1` | zakres `log2` luminancji histogramu |
| `auto.low_percentile` / `high_percentile` | 0.10 / 0.95 | 0…0.9 / 0.1…1 | odcięte końce histogramu |
| `auto.target_luminance` | 0.18 | 0.001…1 | jasność, do której trafia średnia scena |
| `auto.to_brighter_seconds` / `to_darker_seconds` | 0.3 / 1.0 | 0…60 | stałe czasu adaptacji; 0 = od razu |
| `bloom.enabled` | `true` | — | |
| `bloom.intensity` | 0.15 | 0…4 | udział bloomu dodany do obrazu; 0 wyłącza efekt |
| `bloom.threshold` / `knee` | 1.0 / 0.5 | 0…10000 | próg jasności po ekspozycji i miękkość przejścia |
| `bloom.scatter` | 0.7 | 0…1 | ile każdego szerszego poziomu trafia do wyniku |
| `bloom.levels` | 6 | 1…8 | poziomy piramidy (zmniejszane do tego, co mieści obraz) |
| `bloom.karis_average` | `true` | — | ważenie `1 / (1 + luma)` na pierwszym downsample przeciw migotaniu |
| `tonemap.operator` | `AcesFitted` | `AcesFitted`, `PbrNeutral` | krzywa |
| `tonemap.output` | `Srgb` | `Srgb`; `Pq`, `ScRgb` jeszcze nie | transfer wyjściowy (punkt rozszerzenia M4-16, Q0002) |

## Ekspozycja

Tryb ręczny ustawia mnożnik `2^(compensation − ev)`. Tryb automatyczny liczy histogram `log2` luminancji `hdr` w compute (256 pojemników, rozdzielczość ≈ 0.086 EV, pojemnik 0 na czerń), uśrednia po odcięciu percentyli i dąży do `target_luminance`. Przejście do nowej ekspozycji idzie w przestrzeni `log2` ze stałymi czasu osobno dla jaśniejącej i ciemniejącej sceny; czas bierze się z `ViewCtx::delta_seconds` (host go podaje, rdzeń nie czyta zegara): `delta_seconds = 0` zatrzymuje adaptację, pierwsza klatka celu (i klatka po zmianie rozmiaru lub resecie) skacze od razu do celu. Stan adaptacji to historia `post exposure` (ADR-0011), osobna dla każdego celu.

## Bloom

Piramida w passach renderujących (pełnoekranowy trójkąt na poziom), poziom 0 ma połowę rozmiaru renderu: prefilter (próg z kolanem, opcjonalny Karis average) → 13-tapowy downsample → filtr namiotowy z mieszaniem do ostrzejszego poziomu. Energia wejścia jest zachowana (każdy krok to kombinacja wypukła), z wyjątkiem świadomie nieliniowego Karisa.

## Tonemapping i wyjście

| Operator | Opis |
| --- | --- |
| `AcesFitted` | dopasowanie Hilla do RRT+ODT ACES (przedmnożenie `1/0.6`), filmowy |
| `PbrNeutral` | Khronos PBR Neutral: kolory bazowe bez zmian poniżej ramienia, kompresja tylko świateł |

Kodowanie wyjścia wynika z formatu `output`: `*Srgb` koduje sprzęt, `Rgba8Unorm`, `Bgra8Unorm`, `Rgb10a2Unorm` dostają sRGB OETF w shaderze, formaty zmiennoprzecinkowe dostają wartości liniowe. Wyjście HDR (PQ, scRGB) dochodzi z M4-16.

## Odporność na złe wejście

Każdy odczyt `hdr` sanityzuje piksel (NaN i ujemne → 0, `+Inf` → 65504), a tonemapping interpoluje już oczyszczone teksele, więc zepsuty piksel zostaje jednym czarnym (NaN) albo białym (`Inf`) pikselem i nie zatruwa histogramu, bloomu ani sąsiadów. Kadry złożone wyłącznie z 0, `Inf` albo NaN dają skończoną, dodatnią ekspozycję.

## Wymagania urządzenia

`R32Float` zapisywalny z compute (ekspozycja) i filtrowalny `Rgba16Float` jako cel renderowania (bloom, tonemapping): baseline WebGPU. Bez nich negocjacja wyłącza daną część z F0101.

## `pathon_post::reference`

Bliźniaki CPU matematyki z shaderów (krzywe tonemappingu, sRGB OETF, sanityzacja, histogram, adaptacja, próg bloomu). Testy GPU porównują z nimi wyniki; można ich użyć do własnych testów.

## Testy

- Jednostkowe (`cargo test -p pathon-post`): referencje CPU, ustawienia, wymiary piramidy.
- GPU (`--features gpu-tests`, `tests/chain_gpu.rs`): oba operatory kontra referencja CPU (±2/255, także cel `*Srgb` i `Bgra8Unorm`), tryb ręczny, zbieżność ekspozycji automatycznej do szarej karty (±0.1 EV) i zgodność z modelem adaptacji klatka po klatce, zachowanie energii bloomu (±5% średniej), brak NaN i Inf dla wejścia 0, `Inf`, NaN i mieszanego, oraz golden images `post_chain_aces` i `post_chain_neutral` (gradient 16 EV, kolorowe paski, jasna plamka z bloomem) według [ADR-0025](../adr/0025-golden-images.md).

```sh
cargo test -p pathon-post --features gpu-tests -- --test-threads=1
PATHON_UPDATE_GOLDEN=1 cargo test -p pathon-post --features gpu-tests --test chain_gpu golden   # aktualizacja referencji
```
