# Data notices

This is an independent, unofficial project. It is not affiliated with, approved,
endorsed, or sponsored by the City of Chicago, Divvy, Lyft, or the Chicago
Transit Authority (CTA).

## City of Chicago required disclaimer

The City's [Data Terms of Use](https://www.chicago.gov/city/en/narr/foia/data_disclaimer.html),
under **Use of Data**, require the following notice where a derivative
application can be accessed or downloaded. The wording below is reproduced as
directed by those terms:

> This site provides applications using data that has been modified for use from its original source, www.cityofchicago.org, the official website of the City of Chicago. The City of Chicago makes no claims as to the content, accuracy, timeliness, or completeness of any of the data provided at this site. The data provided at this site is subject to change at any time. It is understood that the data provided at this site is being used at one’s own risk.

## Sources and interpretation

Station data: [Divvy Bicycle Stations, City of Chicago Data Portal](https://data.cityofchicago.org/d/bbyy-e7gq),
attributed by the portal to Divvy and owned by the City of Chicago. The catalog
describes daily updates and includes stations that are not in service.

An alert identifies a change observed by this bot. It does not establish an
opening date or confirm that a station or its charging equipment currently
works. Names are trimmed for display; a trailing asterisk or `charging` in the
short name is interpreted as an electrification signal. This interpretation
is made by the bot. The source does not supply a dedicated electrification
field used by this application. Dock counts are inventory, not available-bike
counts. Historical records remain in the bot's private operational database
to avoid duplicate alerts.

Data provided by Chicago Transit Authority: [bus routes](https://data.cityofchicago.org/d/6uva-a5ei),
[rail lines](https://data.cityofchicago.org/d/xbyr-jnvx), and
[rail stations](https://data.cityofchicago.org/d/3tzw-cg4m). Transit overlays are
geographically filtered, styled, and labeled by this project to help readers
locate bike stations near public transportation. They are approximate context,
not live service or routing information. The catalog periods reviewed on
September 18, 2026 were March 2024, November 2023, and August 2024 respectively.
See the [CTA Developer License Agreement](https://www.transitchicago.com/developers/terms/).

Basemap: [Protomaps](https://protomaps.com/) and
[© OpenStreetMap contributors](https://www.openstreetmap.org/copyright).
Optional imagery: Google Street View; image date may differ from the station
observation date. Third-party data and imagery retain their own terms; this
repository does not grant rights to them.

The name Divvy identifies the system being described. No trademark permission
is represented by this notice. See the [Divvy Data License Agreement](https://divvybikes.com/data-license-agreement)
and the [review and unresolved questions](docs/data-terms-review.md).
