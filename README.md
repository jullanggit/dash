A dashboard with all the functionality I'm missing elsewhere.

## Current Features

### Spotify Ratings

Allow rating spotify songs, and playing them more/less frequently based on those ratings.

#### Storage

Ratings are stored in a plain json log of (track, rating, timestamp).

#### Recording

Ratings are recorded either through this web app, or by adding them to any playlist with a name that is parseable into a float. In the latter case the rating will be taken from the playlist name, and the timestamp from the time added to the playlist.
