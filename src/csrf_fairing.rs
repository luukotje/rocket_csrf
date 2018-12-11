use data_encoding::{BASE64, BASE64URL_NOPAD};
use ring::rand::{SecureRandom, SystemRandom};
use rocket::fairing::{Fairing, Info, Kind};
use rocket::http::uri::{Origin, Uri};
use rocket::http::Cookie;
use rocket::http::Method::{self, *};
use rocket::outcome::Outcome;
use rocket::response::Body::Sized;
use rocket::{Data, Request, Response, Rocket, State};
use std::collections::HashMap;
use std::env;
use std::io::{Cursor, Read};
use std::str::from_utf8;
use time::Duration;

use crypto::CsrfProtection;
use csrf_proxy::CsrfProxy;
use csrf_token::CsrfToken;
use path::Path;
use utils::parse_args;
use {CSRF_COOKIE_NAME, CSRF_FORM_FIELD, CSRF_FORM_FIELD_MULTIPART};

/// Builder for [CsrfFairing](struct.CsrfFairing.html)
///
/// The `CsrfFairingBuilder` type allows for creation and configuration of a [CsrfFairing](struct.CsrfFairing.html), the
/// main struct of this crate.
///
/// # Usage
/// A Builder is created via the [`new`] method. Then you can configure it with others provided
/// methods, and get a [CsrfFairing](struct.CsrfFairing.html) by a call to [`finalize`]
///
/// [`new`]: #method.new
/// [`finalize`]: #method.finalize
///
/// ## Examples
///
/// The following shippet show 'CsrfFairingBuilder' being used to create a fairing protecting all
/// endpoints and redirecting error to `/csrf-violation` and treat them as if they where `GET`
/// request then.
///
/// ```rust,no_run
/// # extern crate rocket;
/// # extern crate rocket_csrf;
/// use rocket_csrf::CsrfFairingBuilder;
/// # use rocket::Rocket;
///
/// # fn main() {
///     Rocket::ignite()
///         .attach(CsrfFairingBuilder::new()
///                 .set_default_target("/csrf-violation".to_owned(), rocket::http::Method::Get)
///                 .finalize().unwrap())
///         //add your routes, other fairings...
///         .launch();
/// # }
/// ```

pub struct CsrfFairingBuilder {
    duration: u64,
    default_target: (String, Method),
    exceptions: Vec<(String, String, Method)>,
    secret: Option<[u8; 32]>,
    auto_insert: bool,
    auto_insert_disable_prefix: Vec<String>,
    auto_insert_max_size: u64,
}

impl CsrfFairingBuilder {
    /// Create a new builder with default values.
    pub fn new() -> Self {
        CsrfFairingBuilder {
            duration: 60 * 60 * 12,
            default_target: (String::from("/"), Get),
            exceptions: Vec::new(),
            secret: None,
            auto_insert: true,
            auto_insert_disable_prefix: Vec::new(),
            auto_insert_max_size: 16 * 1024,
        }
    }

    /// Set the timeout (in seconds) of CSRF tokens generated by the final Fairing. Default timeout
    /// is twelve hour.
    pub fn set_timeout(mut self, timeout: u64) -> Self {
        self.duration = timeout;
        self
    }

    /// Set the default route when an invalide request is catched, you may add a <uri> as a segment
    /// or a param to get the percent-encoded original target. You can also set the method of the
    /// route to which you choosed to redirect.
    ///
    /// # Example
    ///
    ///  ```rust,no_run
    /// # extern crate rocket;
    /// # extern crate rocket_csrf;
    /// use rocket_csrf::CsrfFairingBuilder;
    /// # use rocket::Rocket;
    ///
    /// fn main() {
    ///     rocket::ignite()
    ///         .attach(rocket_csrf::CsrfFairingBuilder::new()
    ///                 .set_default_target("/csrf-violation".to_owned(), rocket::http::Method::Get)
    ///                 .finalize().unwrap())
    ///         //add your routes, other fairings...
    ///         .launch();
    /// }
    pub fn set_default_target(mut self, default_target: String, method: Method) -> Self {
        self.default_target = (default_target, method);
        self
    }

    /// Set the list of exceptions which will not be redirected to the default route, removing any
    /// previously added exceptions, to juste add exceptions use [`add_exceptions`] instead. A route may
    /// contain dynamic parts noted as <name>, which will be replaced in the target route.
    /// Note that this is not aware of Rocket's routes, so matching `/something/<dynamic>` while
    /// match against `/something/static`, even if those are different routes for Rocket. To
    /// circunvence this issue, you can add a (not so) exception matching the static route before
    /// the dynamic one, and redirect it to the default target manually.
    ///
    /// [`add_exceptions`]: #method.add_exceptions
    ///
    /// # Example
    ///
    ///  ```rust,no_run
    /// # extern crate rocket;
    /// # extern crate rocket_csrf;
    /// use rocket_csrf::CsrfFairingBuilder;
    /// # use rocket::Rocket;
    ///
    /// fn main() {
    ///     rocket::ignite()
    ///         .attach(rocket_csrf::CsrfFairingBuilder::new()
    ///                 .set_exceptions(vec![
    ///                     ("/some/path".to_owned(), "/some/path".to_owned(), rocket::http::Method::Post),//don't verify csrf token
    ///                     ("/some/<other>/path".to_owned(), "/csrf-error?where=<other>".to_owned(), rocket::http::Method::Get)
    ///                 ])
    ///                 .finalize().unwrap())
    ///         //add your routes, other fairings...
    ///         .launch();
    /// }
    /// ```
    pub fn set_exceptions(mut self, exceptions: Vec<(String, String, Method)>) -> Self {
        self.exceptions = exceptions;
        self
    }
    /// Add the to list of exceptions which will not be redirected to the default route. See
    /// [`set_exceptions`] for more informations on how exceptions work.
    ///
    /// [`set_exceptions`]: #method.set_exceptions
    pub fn add_exceptions(mut self, exceptions: Vec<(String, String, Method)>) -> Self {
        self.exceptions.extend(exceptions);
        self
    }

    /// Set the secret key used to generate secure cryptographic tokens. If not set, rocket_csrf
    /// will attempt to get the secret used by Rocket for it's own private cookies via the
    /// ROCKET_SECRET_KEY environment variable, or will generate a new one at each restart.
    /// Having the secret key set (via this or Rocket environment variable) allow tokens to keep
    /// their validity in case of an application restart.
    ///
    /// # Example
    ///
    ///  ```rust,no_run
    /// # extern crate rocket;
    /// # extern crate rocket_csrf;
    /// use rocket_csrf::CsrfFairingBuilder;
    /// # use rocket::Rocket;
    ///
    /// fn main() {
    ///     rocket::ignite()
    ///         .attach(rocket_csrf::CsrfFairingBuilder::new()
    ///                 .set_secret([0;32])//don't do this, use trully secret array instead
    ///                 .finalize().unwrap())
    ///         //add your routes, other fairings...
    ///         .launch();
    /// }
    /// ```
    pub fn set_secret(mut self, secret: [u8; 32]) -> Self {
        self.secret = Some(secret);
        self
    }

    /// Set if this should modify response to insert tokens automatically in all forms. If true,
    /// this will insert tokens in all forms it encounter, if false, you will have to add them via
    /// [CsrfFairing](struct.CsrfFairing.html), which you may obtain via request guards.
    ///
    pub fn set_auto_insert(mut self, auto_insert: bool) -> Self {
        self.auto_insert = auto_insert;
        self
    }

    /// Set prefixs for which this will not try to add tokens in forms. This has no effect if
    /// auto_insert is set to false. Not having to parse response on paths witch don't need it may
    /// improve performances, but not that only html documents are parsed, so it's not usefull to
    /// use it on routes containing only images or stillsheets.
    pub fn set_auto_insert_disable_prefix(mut self, auto_insert_prefix: Vec<String>) -> Self {
        self.auto_insert_disable_prefix = auto_insert_prefix;
        self
    }

    /// Set the maximum size of a request before it get send chunked. A request will need at most
    /// this additional memory for the buffer used to parse and tokens into forms. This have no
    /// effect if auto_insert is set to false. Default value is 16Kio
    pub fn set_auto_insert_max_chunk_size(mut self, chunk_size: u64) -> Self {
        self.auto_insert_max_size = chunk_size;
        self
    }

    /// Get the fairing from the builder.
    pub fn finalize(self) -> Result<CsrfFairing, ()> {
        let secret = self.secret.unwrap_or_else(|| {
            //use provided secret if one is
            env::vars()
                .find(|(key, _)| key == "ROCKET_SECRET_KEY")
                .and_then(|(_, value)| {
                    let b64 = BASE64.decode(value.as_bytes());
                    if let Ok(b64) = b64 {
                        if b64.len() == 32 {
                            let mut array = [0; 32];
                            array.copy_from_slice(&b64);
                            Some(array)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })//else get secret environment variable
                .unwrap_or_else(|| {
                    eprintln!("[rocket_csrf] No secret key was found, you should consider set one to keep token validity across application restart");
                    let rand = SystemRandom::new();
                    let mut array = [0;32];
                    rand.fill(&mut array).unwrap();
                    array
                }) //if environment variable is not set, generate a random secret and print a warning
        });

        let default_target = Path::from(&self.default_target.0);
        let mut hashmap = HashMap::new();
        hashmap.insert("uri", "".to_owned());
        if default_target.map(&hashmap).is_none() {
            return Err(());
        } //verify if this path is valid as default path, i.e. it have at most one dynamic part which is <uri>
        Ok(CsrfFairing {
            duration: self.duration,
            default_target: (default_target, self.default_target.1),
            exceptions: self
                .exceptions
                .iter()
                .map(|(a, b, m)| (Path::from(&a), Path::from(&b), *m))//TODO verify if source and target are compatible
                .collect(),
            secret,
            auto_insert: self.auto_insert,
            auto_insert_disable_prefix: self.auto_insert_disable_prefix,
            auto_insert_max_size: self.auto_insert_max_size,
        })
    }
}

impl Default for CsrfFairingBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Fairing to protect against Csrf attacks.
///
/// The `CsrfFairing` type protect a Rocket instance against Csrf attack by requesting mendatory
/// token on any POST, PUT, DELETE or PATCH request.
/// This is created via a [CsrfFairingBuilder](struct.CsrfFairingBuilder.html), and implement nothing else than the `Fairing` trait.
///
/// [`CsrfFairingBuilder`]: /rocket_csrf/struct.CsrfFairing.html
pub struct CsrfFairing {
    duration: u64,
    default_target: (Path, Method),
    exceptions: Vec<(Path, Path, Method)>,
    secret: [u8; 32],
    auto_insert: bool,
    auto_insert_disable_prefix: Vec<String>,
    auto_insert_max_size: u64,
}

impl Fairing for CsrfFairing {
    fn info(&self) -> Info {
        if self.auto_insert {
            Info {
                name: "CSRF protection",
                kind: Kind::Attach | Kind::Request | Kind::Response,
            }
        } else {
            Info {
                name: "CSRF protection",
                kind: Kind::Attach | Kind::Request,
            }
        }
    }

    fn on_attach(&self, rocket: Rocket) -> Result<Rocket, Rocket> {
        Ok(rocket.manage((CsrfProtection::from_key(self.secret), self.duration))) //add the Csrf engine to Rocket's managed state
    }

    fn on_request(&self, request: &mut Request, data: &Data) {
        match request.method() {
            Get | Head | Connect | Options => {
                return;
            }
            _ => {}
        };

        {
            let cookies = request.cookies();
            if cookies.iter().count() == 0 {
                return;
            }
        }

        let (csrf_engine, _) = request
            .guard::<State<(CsrfProtection, u64)>>()
            .unwrap()
            .inner();

        let mut cookie = request
            .cookies()
            .get(CSRF_COOKIE_NAME)
            .and_then(|cookie| BASE64URL_NOPAD.decode(cookie.value().as_bytes()).ok());
        let cookie = cookie.as_mut().and_then(|c| csrf_engine.parse_cookie(&mut *c).ok()); //get and parse Csrf cookie

        let mut token = if request
            .content_type()
            .map(|c| c.media_type())
            .filter(|m| m.top() == "multipart" && m.sub() == "form-data")
            .is_some()
        {
            data.peek().split(|&c| c==0x0A || c==0x0D)//0x0A=='\n', 0x0D=='\r'
                .filter(|l| !l.is_empty())
                .skip_while(|&l| l != CSRF_FORM_FIELD_MULTIPART && l != &CSRF_FORM_FIELD_MULTIPART[..CSRF_FORM_FIELD_MULTIPART.len()-2])
                .skip(1)
                .map(|token| token.split(|&c| c==10 || c==13).next())
                .next().unwrap_or(None)
        } else {
            parse_args(from_utf8(data.peek()).unwrap_or(""))
                .filter_map(|(key, token)| {
                    if key == CSRF_FORM_FIELD {
                        Some(token.as_bytes())
                    } else {
                        None
                    }
                })
                .next()
        }.and_then(|token| BASE64URL_NOPAD.decode(&token).ok());
        let token = token.as_mut().and_then(|token| csrf_engine.parse_token(&mut *token).ok());

        if let Some(token) = token {
            if let Some(cookie) = cookie {
                if csrf_engine.verify_token_pair(&token, &cookie) {
                    return; //if we got both token and cookie, and they match each other, we do nothing
                }
            }
        }

        //Request reaching here are violating Csrf protection

        for (src, dst, method) in &self.exceptions {
            if let Some(param) = src.extract(&request.uri().to_string()) {
                if let Some(destination) = dst.map(&param) {
                    if let Ok(origin) = Origin::parse_owned(destination) {
                        request.set_uri(origin);
                        request.set_method(*method);
                        return;
                    }
                }
            }
        }

        //if request matched no exception, reroute it to default target

        let uri = request.uri().to_string();
        let uri = Uri::percent_encode(&uri);
        let mut param: HashMap<&str, String> = HashMap::new();
        param.insert("uri", uri.to_string());
        let destination = self.default_target.0.map(&param).unwrap();
        let origin = Origin::parse_owned(destination).unwrap();

        request.set_uri(origin);
        request.set_method(self.default_target.1)
    }

    fn on_response<'a>(&self, request: &Request, response: &mut Response<'a>) {
        if let Some(ct) = response.content_type() {
            if !ct.is_html() {
                return;
            }
        } //if content type is not html, we do nothing

        let uri = request.uri().to_string();
        if self
            .auto_insert_disable_prefix
            .iter()
            .any(|prefix| uri.starts_with(prefix))
        {
            return;
        } //if request is on an ignored prefix, ignore it

        let token = match request.guard::<CsrfToken>() {
            Outcome::Success(t) => {
                response.adjoin_header(request.cookies().get(CSRF_COOKIE_NAME).unwrap());
                t
            } //guard can't add/remove cookies in on_response, add headers manually
            Outcome::Forward(_) => {
                if request.cookies().get(CSRF_COOKIE_NAME).is_some() {
                    response.adjoin_header(
                        &Cookie::build(CSRF_COOKIE_NAME, "")
                            .max_age(Duration::zero())
                            .finish(),
                    );
                }
                return;
            } //guard can't add/remove cookies in on_response, add headers manually
            Outcome::Failure(_) => return,
        }; /* if we can't get a token, leave request unchanged, this probably
            * means the request had no cookies from the begining
            */

        let body = response.take_body(); //take request body from Rocket
        if body.is_none() {
            return;
        } //if there was no body, leave it that way
        let body = body.unwrap();

        if let Sized(body_reader, len) = body {
            if len <= self.auto_insert_max_size {
                //if this is a small enought body, process the full body
                let mut res = Vec::with_capacity(len as usize);
                CsrfProxy::from(body_reader, &token.value())
                    .read_to_end(&mut res)
                    .unwrap();
                response.set_sized_body(Cursor::new(res));
            } else {
                //if body is of known but long size, change it to a stream to preserve memory, by encapsulating it into our "proxy" struct
                let body = body_reader;
                response.set_streamed_body(Box::new(CsrfProxy::from(body, &token.value())));
            }
        } else {
            //if body is of unknown size, encapsulate it into our "proxy" struct
            let body = body.into_inner();
            response.set_streamed_body(Box::new(CsrfProxy::from(body, &token.value())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use {CSRF_COOKIE_NAME, CSRF_FORM_FIELD};
    use rocket::{
        http::{Cookie, Header, Method},
        local::{Client, LocalRequest},
        Rocket,
    };

    fn default_builder() -> CsrfFairingBuilder {
        super::CsrfFairingBuilder::new()
            .set_default_target("/csrf".to_owned(), Method::Get)
            .set_exceptions(vec![(
                "/ex1".to_owned(),
                "/ex1-target".to_owned(),
                Method::Post,
            )])
            .add_exceptions(vec![(
                "/ex2/<dyn>".to_owned(),
                "/ex2-target/<dyn>".to_owned(),
                Method::Get,
            )])
    }

    fn default_rocket(csrf_fairing: CsrfFairing) -> Rocket {
        ::rocket::ignite()
            .mount(
                "/",
                routes![
                    index,
                    post_index,
                    token,
                    csrf,
                    get_ex1,
                    post_ex1,
                    target_ex1,
                    post_ex2,
                    target_ex2,
                    static_route
                ],
            )
            .attach(csrf_fairing)
    }

    fn get_token(client: &Client) -> (String, String) {
        let mut response = client
            .get("/token")
            .cookie(Cookie::new("some", "cookie"))
            .dispatch(); //get token and cookie
        let token = response.body_string().unwrap();
        let cookie = response
            .headers()
            .get("set-cookie")
            .next()
            .unwrap()
            .split(|c| c == '=' || c == ';')
            .nth(1)
            .unwrap()
            .to_owned();
        (token, cookie)
    }

    fn post_token(client: &Client, path: String, token: String, cookie: String) -> LocalRequest {
        let token = if token.len() > 0 {
            let mut t = Vec::new();
            t.append(&mut CSRF_FORM_FIELD.as_bytes().to_vec());
            t.push(0x3D); //'='
            t.append(&mut token.as_bytes().to_vec());
            t
        } else {
            Vec::new()
        };
        let req = client.post(path).body(&token);
        if !cookie.is_empty() {
            req.cookie(Cookie::new(CSRF_COOKIE_NAME, cookie))
        } else {
            req
        }
    }

    #[test]
    fn test_redirection_on_failure() {
        let rocket = default_rocket(default_builder().finalize().unwrap());
        let client = Client::new(rocket).expect("valid rocket instance");

        let mut response = client.post("/").cookie(Cookie::new("some", "cookie")).dispatch(); //violation well detected
        assert_eq!(response.body_string(), Some("violation".to_owned()));

        let mut response = client.post("/ex1").cookie(Cookie::new("some", "cookie")).dispatch(); //redirection on post
        assert_eq!(response.body_string(), Some("target-ex1".to_owned()));

        let mut response = client.post("/ex2/abcd").cookie(Cookie::new("some", "cookie")).dispatch(); //redirection with dyn part
        assert_eq!(response.body_string(), Some("abcd".to_owned()));
    }

    #[test]
    fn test_non_redirection() {
        let rocket = default_rocket(default_builder().finalize().unwrap());
        let client = Client::new(rocket).expect("valid rocket instance");

        let mut response = client.get("/ex1").cookie(Cookie::new("some", "cookie")).dispatch(); //no redirection on get
        assert_eq!(response.body_string(), Some("get-ex1".to_owned()));

        let (token, cookie) = get_token(&client);

        let mut response =
            post_token(&client, "/".to_owned(), token.clone(), cookie.clone()).cookie(Cookie::new("some", "cookie")).dispatch();
        assert_eq!(response.body_string(), Some("success".to_owned()));

        let mut response =
            post_token(&client, "/ex1".to_owned(), token.clone(), cookie.clone()).cookie(Cookie::new("some", "cookie")).dispatch();
        assert_eq!(response.body_string(), Some("post-ex1".to_owned()));

        let mut response = post_token(
            &client,
            "/ex2/some-url".to_owned(),
            token.clone(),
            cookie.clone(),
        ).cookie(Cookie::new("some", "cookie")).dispatch();
        assert_eq!(response.body_string(), Some("valid-dyn-req".to_owned()));
    }

    #[test]
    fn test_token_timeout() {
        let rocket = default_rocket(default_builder().set_timeout(5).finalize().unwrap());
        let client = Client::new(rocket).expect("valid rocket instance");

        let (token, cookie) = get_token(&client);

        let mut response =
            post_token(&client, "/".to_owned(), token.clone(), cookie.clone()).cookie(Cookie::new("some", "cookie")).dispatch();
        assert_eq!(response.body_string(), Some("success".to_owned()));
        ::std::thread::sleep(::std::time::Duration::from_secs(6));

        //access / with timed out token
        let mut response =
            post_token(&client, "/".to_owned(), token.clone(), cookie.clone()).cookie(Cookie::new("some", "cookie")).dispatch();
        assert_eq!(response.body_string(), Some("violation".to_owned()));
    }

    #[test]
    fn test_invalid_token_pair() {
        let rocket1 = default_rocket(default_builder().set_secret([0; 32]).finalize().unwrap());
        let client1 = Client::new(rocket1).expect("valid rocket instance");
        let rocket2 = default_rocket(default_builder().set_secret([0; 32]).finalize().unwrap());
        let client2 = Client::new(rocket2).expect("valid rocket instance");

        let (token, cookie) = get_token(&client1);

        //having only one part fail
        let mut response =
            post_token(&client2, "/".to_owned(), token.clone(), "".to_owned()).cookie(Cookie::new("some", "cookie")).dispatch();
        assert_eq!(response.body_string(), Some("violation".to_owned()));

        let mut response =
            post_token(&client1, "/".to_owned(), "".to_owned(), cookie.clone()).cookie(Cookie::new("some", "cookie")).dispatch();
        assert_eq!(response.body_string(), Some("violation".to_owned()));

        let (token2, _cookie2) = get_token(&client2);

        //having 2 incompatible parts fail
        let mut response =
            post_token(&client1, "/".to_owned(), token2.clone(), cookie.clone()).cookie(Cookie::new("some", "cookie")).dispatch();
        assert_eq!(response.body_string(), Some("violation".to_owned()));
    }

    #[test]
    fn test_multiple_parametters() {
        let rocket = default_rocket(default_builder().finalize().unwrap());
        let client = Client::new(rocket).expect("valid rocket instance");

        let (token, cookie) = get_token(&client);

        let mut body = Vec::new();
        body.append(&mut "key1=value1&".as_bytes().to_vec());
        body.append(&mut CSRF_FORM_FIELD.as_bytes().to_vec());
        body.push(0x3D); //'='
        body.append(&mut token.as_bytes().to_vec());
        body.append(&mut "&key2=value2".as_bytes().to_vec());
        let mut response = client
            .post("/")
            .body(body)
            .cookie(Cookie::new("something", "before"))
            .cookie(Cookie::new(CSRF_COOKIE_NAME, cookie))
            .cookie(Cookie::new("and", "after"))
            .dispatch();

        assert_eq!(response.body_string(), Some("success".to_owned()));
    }

    #[test]
    fn test_multipart() {
        let body_before = "-----------------------------9051914041544843365972754266
Content-Disposition: form-data; name=\"something\"

value
-----------------------------9051914041544843365972754266
Content-Disposition: form-data; name=\"";
        let body_middle = "\"

";
        let body_after = "

-----------------------------9051914041544843365972754266
Content-Disposition: form-data; name=\"hey\"; filename=\"whatsup\"

How are you?

-----------------------------9051914041544843365972754266--";
        let rocket = default_rocket(default_builder().finalize().unwrap());
        let client = Client::new(rocket).expect("valid rocket instance");

        let (token, cookie) = get_token(&client);

        let mut body = Vec::new();
        body.append(&mut body_before.as_bytes().to_vec());
        body.append(&mut CSRF_FORM_FIELD.as_bytes().to_vec());
        body.append(&mut body_middle.as_bytes().to_vec());
        body.append(&mut token.as_bytes().to_vec());
        body.append(&mut body_after.as_bytes().to_vec());
        let mut response = client
            .post("/")
            .header(Header::new(
                "Content-Type",
                "multipart/form-data; boundary=\
                 ---------------------------\
                 9051914041544843365972754266",
            ))
            .body(body)
            .cookie(Cookie::new(CSRF_COOKIE_NAME, cookie.clone()))
            .cookie(Cookie::new("some", "cookie"))
            .dispatch();

        assert_eq!(response.body_string(), Some("success".to_owned()));
        let mut body = Vec::new();
        body.append(&mut body_before.as_bytes().to_vec());
        body.append(&mut CSRF_FORM_FIELD.as_bytes().to_vec());
        body.append(&mut body_middle.as_bytes().to_vec());
        body.append(&mut "not_a_token".as_bytes().to_vec());
        body.append(&mut body_after.as_bytes().to_vec());
        let mut response = client
            .post("/")
            .header(Header::new(
                "Content-Type",
                "multipart/form-data; boundary=\
                 ---------------------------\
                 9051914041544843365972754266",
            ))
            .body(body)
            .cookie(Cookie::new(CSRF_COOKIE_NAME, cookie))
            .cookie(Cookie::new("some", "cookie"))
            .dispatch();

        assert_eq!(response.body_string(), Some("violation".to_owned()));
    }

    #[test]
    fn test_token_insertion() {
        let rocket = default_rocket(
            default_builder()
                .set_auto_insert_disable_prefix(vec!["/static".to_owned()])
                .finalize()
                .unwrap(),
        );
        let client = Client::new(rocket).expect("valid rocket instance");

        let mut response = client
            .get("/")
            .cookie(Cookie::new("some", "cookie"))
            .dispatch(); //token well inserted
        assert!(
            response.body_string().unwrap().len()
                > "<div><form method='POST'></form></div>".len()
                    + "<input type=\"hidden\" name=\"csrf-token\" value=\"\"/>".len()
        );

        let mut response = client
            .get("/static/something")
            .cookie(Cookie::new("some", "cookie"))
            .dispatch(); //url well ignored by token inserter
        assert_eq!(
            response.body_string(),
            Some("<div><form method='POST'></form></div>".to_owned())
        );
    }

    #[test]
    fn test_auto_insert_disabled() {
        let rocket = default_rocket(default_builder().set_auto_insert(false).finalize().unwrap());
        let client = Client::new(rocket).expect("valid rocket instance");

        let mut response = client
            .get("/")
            .cookie(Cookie::new("some", "cookie"))
            .dispatch();
        assert_eq!(
            response.body_string(),
            Some("<div><form method='POST'></form></div>".to_owned())
        );
    }

    #[test]
    fn test_auto_insert_stream() {
        let rocket = default_rocket(
            default_builder()
                .set_auto_insert_max_chunk_size(1)
                .finalize()
                .unwrap(),
        );
        let client = Client::new(rocket).expect("valid rocket instance");

        let mut response = client
            .get("/")
            .cookie(Cookie::new("some", "cookie"))
            .dispatch(); //token well inserted
        assert!(
            response.body_string().unwrap().len()
                > "<div><form method='POST'></form></div>".len()
                    + "<input type=\"hidden\" name=\"csrf-token\" value=\"\"/>".len()
        );

        //TODO test stream body
    }

    #[test]
    fn test_key_from_env() {
        env::set_var(
            "ROCKET_SECRET_KEY",
            "BAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        );

        let rocket1 = default_rocket(default_builder().finalize().unwrap());
        let client1 = Client::new(rocket1).expect("valid rocket instance");
        let rocket2 = default_rocket(default_builder().finalize().unwrap());
        let client2 = Client::new(rocket2).expect("valid rocket instance");

        let (_token, _cookie) = get_token(&client1);
        let (token2, cookie2) = get_token(&client2);

        //client 1 and 2 should be compatible
        let mut response =
            post_token(&client1, "/".to_owned(), token2.clone(), cookie2.clone()).cookie(Cookie::new("some", "cookie")).dispatch();
        assert_eq!(response.body_string(), Some("success".to_owned()));
    }

    #[test]
    fn test_invalid_default_target() {
        assert!(
            default_builder()
                .set_default_target("/<invalid>".to_owned(), Method::Get)
                .finalize()
                .is_err()
        );
        assert!(
            default_builder()
                .set_default_target("/<uri>".to_owned(), Method::Get)
                .finalize()
                .is_ok()
        );
    }

    #[test]
    fn test_insert_only_on_session() {
        let rocket = default_rocket(default_builder().finalize().unwrap());
        let client = Client::new(rocket).expect("valid rocket instance");

        let mut response = client.get("/").dispatch();
        assert_eq!(response.body_string().unwrap(), "<div><form method='POST'></form></div>");
        assert!(response.headers().get("set-cookie").next().is_none()); // nothing inserted if no session detected

        let mut response = client
            .get("/")
            .cookie(Cookie::new(CSRF_COOKIE_NAME, ""))
            .dispatch();
        assert_eq!(response.body_string().unwrap(), "<div><form method='POST'></form></div>");
        assert!(
            response
                .headers()
                .get_one("set-cookie")
                .unwrap()
                .contains("Max-Age=0")
        ) // delete cookie if no longer in session
    }

    #[test]
    fn test_allow_request_without_session() {
        let rocket = default_rocket(default_builder().finalize().unwrap());
        let client = Client::new(rocket).expect("valid rocket instance");

        let mut response = client.post("/").dispatch();
        assert_eq!(response.body_string().unwrap(), "success");
    }

    //Routes for above test
    #[get("/")]
    fn index() -> ::rocket::response::content::Content<&'static str> {
        ::rocket::response::content::Content(
            ::rocket::http::ContentType::HTML,
            "<div><form method='POST'></form></div>",
        )
    }

    #[post("/")]
    fn post_index() -> &'static str {
        "success"
    }

    #[get("/token")]
    fn token(t: CsrfToken) -> String {
        ::std::str::from_utf8(t.value()).unwrap().to_owned()
    }

    #[get("/csrf")]
    fn csrf() -> &'static str {
        "violation"
    }

    #[get("/ex1")]
    fn get_ex1() -> &'static str {
        "get-ex1"
    }

    #[post("/ex1")]
    fn post_ex1() -> &'static str {
        "post-ex1"
    }

    #[post("/ex1-target")]
    fn target_ex1() -> &'static str {
        "target-ex1"
    }

    #[post("/ex2/<_dyn>")]
    fn post_ex2(_dyn: String) -> &'static str {
        "valid-dyn-req"
    }

    #[get("/ex2-target/<pathpart>")]
    fn target_ex2(pathpart: String) -> String {
        pathpart
    }

    #[get("/static/something")]
    fn static_route() -> ::rocket::response::content::Content<&'static str> {
        ::rocket::response::content::Content(
            ::rocket::http::ContentType::HTML,
            "<div><form method='POST'></form></div>",
        )
    }
}
