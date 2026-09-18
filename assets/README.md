# Rendering assets

Cards use Bikeshare Station headings with no top-left watermark, the original
OPEN/DEPLOYED/CHARGED display wording, and one line of credits at the bottom.
The former official Divvy logo and branded preview files have been removed
from the current tree;
their historical use did not establish permission. See the
[data terms review](../docs/data-terms-review.md).

`fonts/BigShouldersText-Variable.ttf` is the City of Chicago Design System's
primary municipal typeface for social media. The civic concept also uses the
guide's cyan and red accent pairing. The font is distributed by Google Fonts
under the SIL Open Font License included in `fonts/OFL.txt`.

The native Rust renderer embeds static weight-900 (`BigShouldersText-Bold.ttf`)
and weight-600 (`BigShouldersText-Medium.ttf`) instances generated from the
original variable font with FontTools `varLib.instancer`. This avoids the
rasterizer falling back to the lightest variation. They retain the original
SIL Open Font License; Python/FontTools is not a build or runtime dependency.

`announcement-card-rust-civic.jpg` and `announcement-card-rust-nightline.jpg`
are native renderer examples at State St & Randolph St. The original card
examples remain in Git history.
