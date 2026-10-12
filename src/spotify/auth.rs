use super::Error;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{Rng, distr::Alphanumeric};
use reqwest::Url;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{io::ErrorKind, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::JoinHandle,
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
/// How long a browser connection may take to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the login page may stay open before the login gives up.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
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
    pub(crate) refresh: String,
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
        // A closed browser tab never comes back, so the welcome screen needs an end.
        let code = tokio::time::timeout(LOGIN_TIMEOUT, self.wait_for_code(listener))
            .await
            .map_err(|_elapsed| {
                Error::Callback("the browser did not come back in time; try again".into())
            })??;
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

    /// Answers browser requests until one carries this session's response.
    ///
    /// Each connection is read on its own task: a browser may open idle sockets
    /// ahead of time or reset them, and neither may hold up or end the login.
    async fn wait_for_code(&self, listener: TcpListener) -> Result<String, Error> {
        let (found, mut callbacks) = mpsc::unbounded_channel();
        let mut accepting = AbortOnDrop(tokio::spawn(accept(listener, found)));
        while let Some((mut stream, callback)) = callbacks.recv().await {
            let Some(code) = self.code(&callback)? else {
                // An older login page, reloaded: this session can still finish.
                let _ = stream.write_all(STALE).await;
                continue;
            };
            // The code is all the login needs, even if the tab is already gone.
            let _ = respond(&mut stream).await;
            return Ok(code);
        }
        Err(match (&mut accepting.0).await {
            Ok(error) => error.into(),
            Err(error) => error.into(),
        })
    }

    /// Returns the code, or `None` for a response meant for another session.
    fn code(&self, callback: &Url) -> Result<Option<String>, Error> {
        let parameter = |name| {
            callback
                .query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        if parameter("state").as_deref() != Some(self.state.as_str()) {
            return Ok(None);
        }
        if let Some(reason) = parameter("error") {
            return Err(Error::Callback(format!(
                "Spotify declined the login ({reason})"
            )));
        }
        parameter("code")
            .map(Some)
            .ok_or_else(|| Error::Callback("the login response had no code".into()))
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
        // Only a rejected grant means the login is gone; a busy or failing server
        // (429, 5xx) is a network problem, and retrying later is enough.
        if matches!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::UNAUTHORIZED
        ) {
            return Err(Error::Unauthorized);
        }
        let response = response.error_for_status()?;
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

const STALE: &[u8] = b"HTTP/1.1 409 Conflict\r\nContent-Type: text/plain\r\n\
    Content-Length: 50\r\nConnection: close\r\n\r\n\
    This login page is outdated; use the newest one.\r\n";

/// Aborts a background task once nothing waits for it, freeing what it holds.
struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Hands every callback request to `found`; returns only when the listener breaks.
async fn accept(
    listener: TcpListener,
    found: mpsc::UnboundedSender<(TcpStream, Url)>,
) -> std::io::Error {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(serve(stream, found.clone()));
            }
            // Linux reports a connection that failed before being accepted here.
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::ConnectionAborted
                        | ErrorKind::ConnectionReset
                        | ErrorKind::Interrupted
                ) => {}
            Err(error) => return error,
        }
    }
}

/// Reads one browser request, passing it on only when it is the callback.
async fn serve(mut stream: TcpStream, found: mpsc::UnboundedSender<(TcpStream, Url)>) {
    match tokio::time::timeout(REQUEST_TIMEOUT, read_callback(&mut stream)).await {
        Ok(Ok(Some(callback))) => {
            let _ = found.send((stream, callback));
        }
        Ok(Ok(None)) => {
            let _ = stream.write_all(NOT_FOUND).await;
        }
        // A socket that stays silent or breaks is not the login.
        Ok(Err(_)) | Err(_) => {}
    }
}

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

#[cfg(test)]
mod tests {
    use super::Authorization;
    use std::net::SocketAddr;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    async fn get(address: SocketAddr, target: &str) -> String {
        let mut stream = TcpStream::connect(address).await.expect("connects");
        let request = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        stream.write_all(request.as_bytes()).await.expect("sends");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .await
            .expect("reads the answer");
        response
    }

    #[test]
    fn stray_connections_do_not_end_the_login() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a test runtime");
        runtime.block_on(async {
            let authorization = Authorization::new();
            let state = authorization.state.clone();
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
            let address = listener.local_addr().expect("has an address");
            let waiting =
                tokio::spawn(async move { authorization.wait_for_code(listener).await.ok() });
            // A preconnected socket that never speaks, and one that closes at once.
            let _idle = TcpStream::connect(address).await.expect("connects");
            drop(TcpStream::connect(address).await.expect("connects"));
            assert!(
                get(address, "/favicon.ico")
                    .await
                    .starts_with("HTTP/1.1 404")
            );
            let stale = get(address, "/login?code=old&state=other").await;
            assert!(stale.starts_with("HTTP/1.1 409"));
            assert!(stale.ends_with("newest one.\r\n"));
            let done = get(address, &format!("/login?code=fresh&state={state}")).await;
            assert!(done.starts_with("HTTP/1.1 200"));
            assert_eq!(
                waiting.await.expect("the login task finishes").as_deref(),
                Some("fresh")
            );
        });
    }
}
