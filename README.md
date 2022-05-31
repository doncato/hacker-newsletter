# hacker-newsletter
Get top News from Hacker News sent to your Email

# Installation
Download the binary from the release page, or clone this repository and build
it from source if you have rust installed.

# Usage
Execute the binary, in which case the config,
database and log file should be generated. As the config will contain default
values the application will almost certainly crash right away.
With the generated configuration it is your job to fit it to your setup.

The configuration contains the following fields:

| Field             | Description                                       |
| ----------------- | ------------------------------------------------- |
| email_domain      | The domain of the SMTP server                     |
| email_user        | The user to authenticate with the SMTP server     |
| email_pass        | The password to authenticate with the SMTP server |
| database_path     | The path to the sqlite database file              |
| content_html_path | The path to the scheme file for the email content |
| unsubscribe_url   | The URL to unsubscribe from the newsletter        |
| log_path          | The path to the log file                          |

**Notes:**
- Upon execution the application will directly generate and send the emails, to get an actual newsletter
    you could run this app in a cronjob or something similar.
- The unsubscribe url is just blindly sent in the email along with the recipients address
    (e.g. if unsubscribe_url is set to `https://example.com/unsubscribe?email=` then the unsubscribe link in
    the email will point to `https://example.com/unsubscribe?email=recipient@example.com`)
- The Application will attempt to communicate with the SMTP server over `STARTTLS` on Port `587`

**Database:**
- The Database table is called `users` and has 2 columns: `email` and `count`
- The `email` column is the primary key and describes the email address of a user
- The `count` column describes how many posts should be sent in one notification. The Maxmimum is 255
