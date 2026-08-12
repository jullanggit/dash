### Spotify accounts

Spotify accounts are configured in a separate secrets file, whose location is configured in the general config.

```json
{
  "spotify": {
    "secretsFile": "~/.config/dash/spotify-secrets.json"
  }
}
```

The secrets file contains the Spotify app credentials for each account:

```json
{
  "accounts": {
    "primary": {
      "clientId": "...",
      "clientSecret": "..."
    },
    "secondary": {
      "clientId": "...",
      "clientSecret": "..."
    }
  }
}
```

Access and refresh tokens are obtained at runtime by rspotify and stored in a directory, whose location is configured in the general config.
All accounts have to be authenticated for the same Spotify user and are assumed to be interchangeable.

```json
{
  "spotify": {
    "tokenCacheDirectory": "~/.cache/dash/rspotify"
  }
}
```
