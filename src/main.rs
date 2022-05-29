use chrono::{self, Local};
use confy;
use env_logger::Builder;
use lettre::{
    smtp::{
        authentication::{Credentials, Mechanism},
        ConnectionReuseParameters,
    },
    ClientSecurity, ClientTlsParameters, EmailAddress, Envelope, SendableEmail, SmtpClient,
    SmtpTransport, Transport,
};
use log::LevelFilter;
use native_tls::{Protocol, TlsConnector};
use reqwest::blocking::Client;
use rusqlite::Connection;
use serde_derive::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const PAGE_URL: &'static str = "https://news.ycombinator.com/item?id=";
const POSTLIST_URL: &'static str = "https://hacker-news.firebaseio.com/v0/topstories.json";
const POST_URL: &'static str = "https://hacker-news.firebaseio.com/v0/item/";
const HTML_LINE: &'static str = "<li><a href=\"{PLACE:URL}\">{PLACE:TITLE}</a><br>&emsp;by {PLACE:BY} | {PLACE:SCORE} points</li>";

#[derive(Serialize, Deserialize)]
struct AppConfig {
    email_domain: String,
    email_user: String,
    email_pass: String,
    database_path: PathBuf,
    content_html_path: PathBuf,
    unsubscribe_url: String,
}
impl ::std::default::Default for AppConfig {
    fn default() -> Self {
        Self {
            email_domain: "localhost".to_string(),
            email_user: "".to_string(),
            email_pass: "".to_string(),
            database_path: Path::new("./newsletter.sqlite").to_path_buf(),
            content_html_path: Path::new("./message.html").to_path_buf(),
            unsubscribe_url: "localhost/unsubscribe/?email=".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
struct Post {
    id: u32,
    by: String,
    url: String,
    score: i16,
    title: String,
}
impl Post {
    fn new(id: u32, by: String, url: String, score: i16, title: String) -> Self {
        Self {
            id,
            by,
            url,
            score,
            title,
        }
    }
    fn empty() -> Self {
        Self {
            id: 0,
            by: "".to_string(),
            url: "".to_string(),
            score: 0,
            title: "".to_string(),
        }
    }

    fn is_empty(&self) -> bool {
        self == &Self::empty()
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
struct PartialPost {
    id: u32,
    by: String,
    url: Option<String>,
    score: i16,
    title: String,
}
impl PartialPost {
    fn empty() -> Self {
        Self::from_post(Post::empty())
    }
    fn to_post(self) -> Post {
        let url = match self.url {
            Some(url) => url,
            None => format!("{}{}", PAGE_URL, self.id),
        };
        Post::new(self.id, self.by, url, self.score, self.title)
    }
    fn from_post(post: Post) -> Self {
        Self {
            id: post.id,
            by: post.by,
            url: Some(post.url),
            score: post.score,
            title: post.title,
        }
    }
}

#[derive(Clone)]
struct User {
    email: String,
    count: u8,
}
impl User {
    fn empty() -> Self {
        Self {
            email: "".to_string(),
            count: 10,
        }
    }
}

fn close_database(mut database: Connection, retries: u8) -> Result<(), ()> {
    for attempt in 0..retries {
        match database.close() {
            Ok(()) => return Ok(()),
            Err(err) => {
                log::warn!(
                    "Failed to close the database!{}\nRetrying ({}/{})",
                    err.1,
                    attempt,
                    retries
                );
                database = err.0;
                continue;
            }
        }
    }
    return Err(());
}

fn create_database(database: &Connection) -> Result<(), rusqlite::Error> {
    database.execute(
        "CREATE TABLE IF NOT EXISTS users (email STRING PRIMARY KEY, count INTEGER)",
        [],
    )?;
    Ok(())
}

fn get_all_users(database: &Connection) -> Result<Vec<User>, rusqlite::Error> {
    let mut query = database.prepare("SELECT email, count FROM users")?;
    let users = query.query_map([], |row| {
        Ok(User {
            email: row.get(0).unwrap_or("".to_string()),
            count: row.get(1).unwrap_or(10),
        })
    })?;

    Ok(users.map(|e| e.unwrap_or(User::empty())).collect())
}

fn get_config() -> Result<AppConfig, confy::ConfyError> {
    confy::load_path("./newsletter.config")
}

fn get_postlist(client: &Client, count: u8) -> Vec<u32> {
    match client.get(POSTLIST_URL).send() {
        Ok(response) => {
            let mut content = match response.json::<Vec<u32>>() {
                Ok(val) => val,
                Err(err) => {
                    log::warn!("Failed to get posts: {}", err);
                    Vec::new()
                }
            };
            content.truncate(count as usize);
            return content;
        }
        Err(err) => {
            log::error!("Failed to get posts: {:#?}", err);
            return Vec::new();
        }
    };
}

fn get_posts(client: &Client, count: u8) -> Vec<Post> {
    let list = get_postlist(client, count);
    if list.is_empty() {
        log::error!("No posts available! Nothing to send to the users!");
        return Vec::new();
    }

    list.iter()
        .map(
            |id| match client.get(format!("{}{}.json", POST_URL, id)).send() {
                Ok(response) => match response.json::<PartialPost>() {
                    Ok(val) => val,
                    Err(err) => {
                        log::warn!("Error while getting post {}: {}", id, err);
                        PartialPost::empty()
                    }
                }
                .to_post(),

                Err(err) => {
                    log::warn!("Error while getting post {}: {}", id, err);
                    Post::empty()
                }
            },
        )
        .filter(|post| !post.is_empty())
        .collect()
}

fn init_logger() {
    Builder::new()
        .format(|buf, record| {
            writeln!(
                buf,
                "[{}] {} - {}: {}",
                record.level(),
                Local::now().format("%d/%m/%y %H:%M:%S"),
                record.target(),
                record.args(),
            )
        })
        .filter(None, LevelFilter::Debug)
        //.filter(None, LevelFilter::Info)
        .init();
}

fn send_news(
    smtp: &mut SmtpTransport,
    email: &String,
    posts: &[Post],
    html: &str,
    cfg: &AppConfig,
) -> Result<(), ()> {
    let elements: Vec<String> = posts
        .iter()
        .map(|post| {
            HTML_LINE
                .replace("{PLACE:URL}", &post.url)
                .as_str()
                .replace("{PLACE:TITLE}", &post.title)
                .as_str()
                .replace("{PLACE:BY}", &post.by)
                .as_str()
                .replace("{PLACE:SCORE}", &post.score.to_string())
        })
        .collect();
    let message = html
        .replace("{PLACE:RECIPIENT}", &email)
        .as_str()
        .replace("{PLACE:ELEMENT}", &elements.join("\n"))
        .as_str()
        .replace("{PLACE:UNSUBSCRIBE_URL}", &cfg.unsubscribe_url);

    let sender = match EmailAddress::new(cfg.email_user.clone()) {
        Ok(addr) => addr,
        Err(_) => {
            log::error!(
                "Failed to send email! '{}' not a vaild sender address",
                cfg.email_user
            );
            return Err(());
        }
    };

    let address = match EmailAddress::new(email.clone()) {
        Ok(addr) => addr,
        Err(_) => {
            log::error!(
                "Failed to send email! '{}' not a vaild recipient address",
                email
            );
            return Err(());
        }
    };
    let envelope = match Envelope::new(Some(sender), vec![address]) {
        Ok(envlp) => envlp,
        Err(e) => {
            log::error!("Failed to send email: {}", e);
            return Err(());
        }
    };
    let mail = SendableEmail::new(envelope, "id-00".to_string(), message.into_bytes());
    match smtp.send(mail) {
        Ok(_) => Ok(()),
        Err(e) => {
            log::error!("Failed to send email: {}", e);
            Err(())
        }
    }
}

fn main() {
    init_logger();
    let cfg = get_config().expect("Failed to read config file!");
    log::debug!("Read config");
    let db = Connection::open(&cfg.database_path).expect("Failed to open database!");
    log::debug!("Opened Connection with database");
    if create_database(&db).is_err() {
        log::warn!("Failed to safely create database! Proceeding anyway...");
    }
    let users = get_all_users(&db).expect("Failed to fetch emails from database!");
    log::debug!("Got users from database");
    if close_database(db, 5).is_err() {
        log::warn!("Failed to close database! No retries left. Proceeding anyway...");
    };
    if users.is_empty() {
        log::info!("No users were found! Nobody to send anything to. Exiting...");
        return;
    }

    let html = fs::read_to_string(cfg.content_html_path.clone())
        .expect("Failed to read content html file! Therefore I don't know what to send!");

    let creds = Credentials::new(cfg.email_user.clone(), cfg.email_pass.clone());
    let tls_parameters = ClientTlsParameters::new(
        cfg.email_domain.clone(),
        TlsConnector::builder()
            .min_protocol_version(Some(Protocol::Tlsv10))
            .build()
            .expect("Failed to build TLS Connection!"),
    );
    let mut smtp = SmtpClient::new(
        (cfg.email_domain.as_str(), 587),
        ClientSecurity::Required(tls_parameters),
    )
    .expect("Failed to connect to SMTP Server!")
    .authentication_mechanism(Mechanism::Login)
    .credentials(creds)
    .connection_reuse(ConnectionReuseParameters::ReuseUnlimited)
    .transport();
    log::debug!("Connected to the SMTP Server");

    let highest_count = users
        .clone()
        .into_iter()
        .map(|user| user.count)
        .into_iter()
        .max()
        .unwrap_or(10);

    let client = reqwest::blocking::Client::new();
    let posts = get_posts(&client, highest_count);
    if posts.is_empty() {
        panic!("No posts could be fetched! I have nothing I could send to the users!");
    }
    log::debug!("Fetched {} posts", posts.len());

    for user in users.iter() {
        match send_news(
            &mut smtp,
            &user.email,
            &posts[..user.count as usize],
            html.as_str(),
            &cfg,
        ) {
            Ok(()) => log::info!("Sent Email to {}", &user.email),
            Err(()) => log::warn!("Failed to send Email to {}", &user.email),
        };
    }
    smtp.close();
}
