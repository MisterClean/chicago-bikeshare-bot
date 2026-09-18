# Data terms review — September 18, 2026

This is a practical review of the project's data use and notices, not a legal
opinion or a certification of compliance. Production, its credentials, the
historical database, existing posts, and the Bluesky profile were not changed.

## Chicago: a required notice was missing

The portal footer links to the City's [Data Terms of Use](https://www.chicago.gov/city/en/narr/foia/data_disclaimer.html).
The **Use of Data** section requires its specified disclaimer wherever a
derivative application can be accessed or downloaded, and compliance with
additional contributing-agency terms. The terms also reserve withdrawal and
intellectual-property rights and impose indemnity/defense obligations.

The exact required notice is now in the repository README and [NOTICE.md](../NOTICE.md).
Carry it into any future website, download page, or package description.
The native release includes it at `/opt/chicago-bikeshare-bot/current/NOTICE.md`; update
the public GHCR package description with the notice when publishing, too.
The existing Bluesky profile already links to this repository, but has no
unofficial/non-affiliation statement. Keep that link pointed at the published
notice and consider a pinned explanatory post. A short image footer or alt text
does not replace the required full notice. This account work is still pending;
no public profile or post was edited during this review.

The official page was fetched successfully over HTTPS for this review even
though the search browser returned 403. The notice above comes from that page,
not an unrelated City website's terms or a third-party summary.

## Divvy: permissions need clarification

The [Divvy Data License Agreement](https://divvybikes.com/data-license-agreement)
permits data analysis and use in applications but restricts standalone dataset
redistribution, unauthorized access/extraction, customer identification, and
implied endorsement. It expressly requires written permission for trademark
and trade-name use, including DIVVY and its logo. A disclaimer does not grant
that permission. The contact given is **bike-data@lyft.com**.

The bot uses the City portal API, not Divvy's trip downloads. The City catalog
credits Divvy and links its GBFS feed; its license field says `SEE_TERMS_OF_USE`.
The [Divvy system-data page](https://divvybikes.com/system-data) attaches the
agreement to trip data and separately links live stations. The precise
application of Divvy's agreement to this City-hosted dataset is unresolved.
Do not assume the agreement is irrelevant, or that all automated analysis is
prohibited: its analysis grant and extraction restriction need clarification.

Ask the City/operator to confirm this API-based change-alert use, any retained
descriptive name/handle use, and any desired logo permission. Do not assume a
noncommercial bot automatically meets every exception. No permission request
was sent. The legacy tracked `data/divvy_stations.db` is also a distribution
question if the repository is public; it was left intact. Avoid adding further
raw data exports or changing Git history while this is unresolved. The private
production incremental database is not a public data-download feature.

## CTA: public-transport context and attribution

The [current CTA agreement](https://www.transitchicago.com/developers/terms/)
covers CTA data made available through feeds or otherwise. Section I limits
licensed use to assisting transit riders or promoting public transportation.
Showing nearby bus and rail connections serves that purpose; this is our
interpretation of the map's use, not a permission ruling from CTA.

Section III prohibits implied endorsement and requires reasonable efforts to
keep cached data current. A credit line is optional; the project uses one.
The renderer fetches the portal layers when rendering instead of maintaining
a separate stale local cache. However, the source catalog itself describes
older vintages, so overlays must not be described as live service. There are
no CTA logos in the renderer. The agreement also contains indemnification and
termination duties; the notice alone is not the entire agreement.

## Data provenance and accuracy

Reviewed the official catalog metadata for all four sources:

| Source | Dataset / metadata | Catalog period or frequency |
| --- | --- | --- |
| Station inventory | [bbyy-e7gq](https://data.cityofchicago.org/api/views/bbyy-e7gq) | Current; daily; includes inactive stations |
| CTA buses | [6uva-a5ei](https://data.cityofchicago.org/api/views/6uva-a5ei) | March 2024 |
| CTA rail lines | [xbyr-jnvx](https://data.cityofchicago.org/api/views/xbyr-jnvx) | November 2023; approximate geography |
| CTA rail stations | [3tzw-cg4m](https://data.cityofchicago.org/api/views/3tzw-cg4m) | August 2024 |

All four metadata responses identify their license as `SEE_TERMS_OF_USE`;
none was represented as CC0 or a blanket public-domain dedication.

The bot's first observation is not a station opening date. Electrification is
an inference from source naming conventions. These are accuracy findings and
recommended wording changes, not claims that the City mandates a particular
alert title. Keep existing event types, deduplication, and historical state.

## Public profile draft

Following the owner's visual revision, local cards use Bikeshare Station
headings and the original OPEN/DEPLOYED/CHARGED wording, without a top-left
watermark. City/CTA, Protomaps, OpenStreetMap, and unofficial credits occupy
one footer line. The official logo and old branded previews are removed from
the current tree, and its SVG rendering dependency is removed. Post text and
alt text retain the inventory/charging qualifications described above; the
image headlines do not establish a verified opening date or equipment status.
Post length handling keeps the notice intact for unusually long station names.
These edits do not change event detection or database behavior.

Suggested display name: **Chicago Bike Station Watch (unofficial)**.

Suggested biography:

> Unofficial Chicago station alerts. Not affiliated with the City, Divvy, Lyft or CTA. Changes may be delayed or inferred; check the official app before riding. Data notices: https://github.com/MisterClean/chicago-bikeshare-bot

Use the repository README link only after these notices are on its public
default branch. A pinned explanation can expand on the daily inventory and
electrification heuristic. Changing the existing handle or deleting old posts
needs a separate decision; this local review does neither.

## Remaining release decisions

- Resolve name/logo permission and the scope of the Divvy data agreement with
  the rights holder or legal advice before describing the project as compliant.
- Apply the profile and notice-page changes when publishing this branch.
- Decide how to handle the already tracked historical dataset and branded
  images in Git history/old posts; no historical database or Git history was
  altered here.
- This review covers Chicago, Divvy, and the CTA layers actually used. Google
  Street View redistribution and Protomaps/OpenStreetMap terms need a separate
  review before claiming compliance for the complete media pipeline. Existing
  watermarks or credits alone do not establish redistribution permission.
