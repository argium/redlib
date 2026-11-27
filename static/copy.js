let resetStatus;

// Reports the result in the status element beside the button and clears it after two seconds.
function report(message) {
    const status = document.getElementById('copy_status');
    clearTimeout(resetStatus);
    status.textContent = message;
    resetStatus = setTimeout(() => { status.textContent = ''; }, 2000);
}

// The Clipboard API is missing on plain HTTP and .onion pages, and rejects when permission is denied.
async function writeToClipboard(field) {
    try {
        await navigator.clipboard.writeText(field.value);
        return true;
    } catch {
        // Selecting the field moves focus to it, so hand focus back afterwards.
        const previouslyFocused = document.activeElement;
        field.select();
        try {
            return document.execCommand('copy');
        } catch {
            return false;
        } finally {
            previouslyFocused?.focus();
        }
    }
}

async function copy() {
    const copied = await writeToClipboard(document.getElementById('bincode_str'));
    report(copied ? 'Copied' : 'Copy failed');
}

async function set_listener() {
    document.getElementById('copy').addEventListener('click', copy);
}

window.addEventListener('load', set_listener);
