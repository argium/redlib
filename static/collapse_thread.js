// @license http://www.gnu.org/licenses/agpl-3.0.html AGPL-3.0
// The line beside a comment's replies collapses the comment, like the line on a threaded forum.
// The summary row keeps working without this script, and for the keyboard.
document.addEventListener('click', function (event) {
    var line = event.target.closest && event.target.closest('.line');
    if (!line) {
        return;
    }

    var details = line.closest('.comment').querySelector('details.comment_right');
    details.open = !details.open;
    if (!details.open) {
        // The header may be far above after a long thread; keep it on screen.
        details.querySelector('summary').scrollIntoView({ block: 'nearest' });
    }
});
// @license-end
