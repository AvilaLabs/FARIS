# Avila Labs account (optional)

The desktop app has an optional Avila Labs account. Every part of FARIS works fully without one. FARIS has no feature that needs an account.

## What you see

At the right end of the top bar there is a grid button that opens the Avila Labs tools, and a **Sign in** button. After you sign in, the button becomes an account chip. The first time you start the app, a prompt may offer to sign in. It does not appear while the tour plays, or in scripted runs.

## Sign in

1. Choose **Sign in**. The sign-in card opens. Choose **Start sign-in**.
2. The card shows a short code and an **Open browser** button. Your browser opens and asks you to approve this device. Check that the page shows the same code.
3. Approve it in the browser. The card reads "Signed in as" followed by your account.

If you deny the sign-in, or the code expires before you approve it, the card says so and offers **Try again**. **Close** leaves the card without signing in.

The app identifies itself to the service as `faris-desktop`. Choose **Sign out** to leave the account. Signing out revokes the token on the service and deletes the local credentials file.

## What is stored and sent

The token is saved in `credentials.json` (mode 0600), in the folder named by `AVILA_CONFIG_DIR`, or in `~/.config/avila` when that is not set. The credentials file also keeps your email address for display.

The app sends the service a sign-in request with the app name, the token during sign-in, and afterwards the token to check your account status about once a minute. Nothing from your FARIS work is sent: no inputs, results or files. The account service's [privacy notice](https://api.avilalabs.org/privacy) has the details.

The roadmap plans to move token storage into the operating system's credential store where one exists, with an owner-only file as the fallback.
