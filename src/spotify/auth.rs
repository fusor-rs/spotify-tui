use super::Error;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{Rng, distr::Alphanumeric};
use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// librespot's client ID: Spotify only lets privileged client IDs stream audio.
const CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
const CALLBACK_ADDRESS: &str = "127.0.0.1:8898";
const CALLBACK_PATH: &str = "/login";
const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const SCOPES: &str = "streaming user-read-private playlist-read-private user-library-read";
const VERIFIER_LENGTH: usize = 64;
const STATE_LENGTH: usize = 16;
const MAX_REQUEST_BYTES: usize = 8192;
const KEYRING_SERVICE: &str = "spotify-tui";
const KEYRING_ACCOUNT: &str = "refresh-token";
const CALLBACK_PAGE: &str = include_str!("../../assets/callback.html");

pub(crate) struct Authorization {
    pub(crate) url: String,
    verifier: String,
    state: String,
}

pub(crate) struct Token {
    pub(crate) access: String,
    refresh: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
}

impl Authorization {
    pub(crate) fn new() -> Self {
        let verifier = random_text(VERIFIER_LENGTH);
        let state = random_text(STATE_LENGTH);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier));
        let url = Url::parse_with_params(
            AUTHORIZE_URL,
            [
                ("client_id", CLIENT_ID),
                ("response_type", "code"),
                ("redirect_uri", &redirect_uri()),
                ("scope", SCOPES),
                ("state", &state),
                ("code_challenge_method", "S256"),
                ("code_challenge", &challenge),
            ],
        )
        .expect("the authorize URL constant is a valid URL");
        Self {
            url: url.into(),
            verifier,
            state,
        }
    }

    /// Waits for the browser to return to the local callback, then trades the code for a token.
    pub(crate) async fn complete(self, http: &reqwest::Client) -> Result<Token, Error> {
        let listener = TcpListener::bind(CALLBACK_ADDRESS).await.map_err(|error| {
            Error::Callback(format!("cannot listen on {CALLBACK_ADDRESS}: {error}"))
        })?;
        let code = loop {
            let (mut stream, _) = listener.accept().await?;
            let Some(callback) = read_callback(&mut stream).await? else {
                stream.write_all(NOT_FOUND).await?;
                continue;
            };
            respond(&mut stream).await?;
            break self.code(&callback)?;
        };
        let redirect = redirect_uri();
        Token::request(
            http,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", &redirect),
                ("client_id", CLIENT_ID),
                ("code_verifier", &self.verifier),
            ],
            None,
        )
        .await
        .map_err(|error| match error {
            Error::Unauthorized => Error::Callback("Spotify rejected the login; try again".into()),
            other => other,
        })
    }

    fn code(&self, callback: &Url) -> Result<String, Error> {
        let parameter = |name| {
            callback
                .query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        if let Some(reason) = parameter("error") {
            return Err(Error::Callback(format!(
                "Spotify declined the login ({reason})"
            )));
        }
        if parameter("state").as_deref() != Some(self.state.as_str()) {
            return Err(Error::Callback(
                "the login response did not match this session".into(),
            ));
        }
        parameter("code").ok_or_else(|| Error::Callback("the login response had no code".into()))
    }
}

impl Token {
    pub(crate) async fn refresh(
        http: &reqwest::Client,
        refresh_token: &str,
    ) -> Result<Self, Error> {
        Self::request(
            http,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", CLIENT_ID),
            ],
            Some(refresh_token),
        )
        .await
    }

    async fn request(
        http: &reqwest::Client,
        form: &[(&str, &str)],
        previous_refresh: Option<&str>,
    ) -> Result<Self, Error> {
        let response = http.post(TOKEN_URL).form(form).send().await?;
        if !response.status().is_success() {
            return Err(Error::Unauthorized);
        }
        let token: TokenResponse = response.json().await?;
        let refresh = token
            .refresh_token
            .or_else(|| previous_refresh.map(str::to_owned))
            .ok_or(Error::Unauthorized)?;
        Ok(Self {
            access: token.access_token,
            refresh,
        })
    }

    pub(crate) fn store(&self) -> Result<(), Error> {
        keyring_entry()?.set_password(&self.refresh)?;
        Ok(())
    }
}

pub(crate) fn stored_refresh_token() -> Result<Option<String>, Error> {
    match keyring_entry()?.get_password() {
        Ok(token) => Ok(Some(token)),
        // Not being logged in yet is the expected first-run state.
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn forget() -> Result<(), Error> {
    match keyring_entry()?.delete_credential() {
        // Logging out twice leaves the intended state.
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn keyring_entry() -> keyring::Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
}

fn redirect_uri() -> String {
    format!("http://{CALLBACK_ADDRESS}{CALLBACK_PATH}")
}

fn random_text(length: usize) -> String {
    rand::rng()
        .sample_iter(Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}

const NOT_FOUND: &[u8] =
    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

/// Returns the callback URL, or `None` for unrelated requests such as a favicon.
async fn read_callback(stream: &mut TcpStream) -> Result<Option<Url>, Error> {
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await?;
        if read == 0 || request.len() > MAX_REQUEST_BYTES {
            return Ok(None);
        }
        request.extend_from_slice(&buffer[..read]);
    }
    let request = String::from_utf8_lossy(&request);
    let Some(target) = request
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("GET "))
        .and_then(|line| line.split(' ').next())
    else {
        return Ok(None);
    };
    let url = Url::parse(&format!("http://{CALLBACK_ADDRESS}{target}"))
        .map_err(|error| Error::Callback(error.to_string()))?;
    Ok((url.path() == CALLBACK_PATH).then_some(url))
}

async fn respond(stream: &mut TcpStream) -> Result<(), Error> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{CALLBACK_PAGE}",
        CALLBACK_PAGE.len()
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}
