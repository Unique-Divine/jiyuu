/**
 * Monaco, eingebunden als ESM ueber Vite. Kein CDN, kein AMD-Loader.
 *
 * **Warum ESM und nicht der AMD-Weg aus spike/.** Der Spike lud Monaco ueber
 * `loader.js` aus `/node_modules/monaco-editor/min/vs`. Das war fuer einen
 * Wegwerf-Beweis richtig und fuer das Produkt falsch: der ausgelieferte Ordner
 * ist `dist/`, und der Server bettet genau das ein, was dort liegt
 * (scripts/embed-frontend.sh sammelt dist/ per find).
 * Ein AMD-Loader haette verlangt, `min/vs` daneben zu kopieren und den Pfad zur
 * Laufzeit zu raten. Als ESM-Import ist Monaco Teil des Bundles, und was Vite
 * gebaut hat, liegt vollstaendig in dist/.
 *
 * **Die Importpfade sind die von 0.56, nicht die aus der alten Anleitung.**
 * Die Vite-Rezepte im Netz schreiben `monaco-editor/esm/vs/editor/editor.api`.
 * Die `exports`-Tabelle von monaco-editor 0.56 bildet `./*` auf `./esm/vs/*.js`
 * ab, also loest genau dieser Pfad auf `esm/vs/esm/vs/editor/editor.api.js` auf
 * und existiert nicht. Richtig ist `monaco-editor/editor/editor.api`. Geprueft
 * mit `import.meta.resolve`, nicht geraten.
 *
 * **Die Sprachdienste fehlen mit Absicht.** Der volle Einstiegspunkt
 * (`import * as monaco from 'monaco-editor'`) zieht die TypeScript-, JSON-,
 * CSS- und HTML-Sprachdienste mit, also einen Typpruefer in einem Worker. Der
 * wuerde hier ueber eine Datei laufen, die der Server bei 500 Zeilen ABGESCHNITTEN
 * geliefert hat, und rote Wellenlinien unter Code malen, der nur deshalb
 * unvollstaendig aussieht. Eine Lese-Oberflaeche, die Fehler erfindet, ist
 * schlimmer als eine ohne Diagnosen.
 *
 * Deshalb `languages/definitions/register.all` und nicht `languages/register.all`:
 * die aeussere Fassung zieht am Ende ihrer Liste die vier Sprachdienste mit
 * herein, die innere sind nur die 81 Monarch-Definitionen. Faerbung ist damit
 * rein lexikalisch und behauptet nichts. Der Preis ist benannt: JSON hat keine
 * Monarch-Definition, weil seine Faerbung beim Sprachdienst liegt, also
 * erscheint eine .json-Datei ungefaerbt. Ungefaerbt ist ehrlich; falsch
 * unterringelt waere es nicht.
 *
 * Uebrig bleibt genau ein Worker, der Basis-Worker des Editors.
 */

import * as monaco from 'monaco-editor/editor/editor.api';
import 'monaco-editor/languages/definitions/register.all';
import EditorWorker from 'monaco-editor/editor/editor.worker?worker';

/** Der Name des Themes, das aus den Tokens gebaut ist. */
export const ATLAS_THEME = 'atlas-phosphor';

let prepared = false;

/**
 * Die Farben des Editors aus denselben Tokens wie das uebrige Chrome.
 *
 * Monaco kann keine CSS-Variablen lesen, also stehen die Werte hier ein zweites
 * Mal. Das ist die eine Doppelung, die dieses Design sich leistet, und sie
 * steht direkt neben tokens.css, damit sie beim naechsten Griff in die Palette
 * mitwandert.
 */
function defineTheme(): void {
    monaco.editor.defineTheme(ATLAS_THEME, {
        base: 'vs-dark',
        inherit: true,
        rules: [
            { token: '', foreground: 'dbe7f1', background: '101720' },
            { token: 'comment', foreground: '8196a8', fontStyle: 'italic' },
            { token: 'keyword', foreground: '63c9eb' },
            { token: 'string', foreground: '8fd6ae' },
            { token: 'number', foreground: 'e0a06a' },
            { token: 'regexp', foreground: 'e0a06a' },
            { token: 'type', foreground: '9be2f1' },
            { token: 'type.identifier', foreground: '9be2f1' },
            { token: 'delimiter', foreground: 'a4b7c6' },
            { token: 'delimiter.bracket', foreground: 'a4b7c6' },
            { token: 'tag', foreground: '9be2f1' },
            { token: 'attribute.name', foreground: '8fd6ae' },
            { token: 'invalid', foreground: 'ff5f8f' },
        ],
        colors: {
            'editor.background': '#101720',
            'editor.foreground': '#dbe7f1',
            'editorGutter.background': '#101720',
            'editorLineNumber.foreground': '#8196a8',
            'editorLineNumber.activeForeground': '#63c9eb',
            'editor.lineHighlightBackground': '#16212e',
            'editor.lineHighlightBorder': '#16212e',
            'editor.selectionBackground': '#193446',
            'editor.inactiveSelectionBackground': '#193446',
            'editorCursor.foreground': '#63c9eb',
            'editorWhitespace.foreground': '#2e3848',
            'editorIndentGuide.background1': '#243344',
            'editorIndentGuide.activeBackground1': '#2e3848',
            'scrollbarSlider.background': '#2e384888',
            'scrollbarSlider.hoverBackground': '#2e3848cc',
            'scrollbarSlider.activeBackground': '#72a8bdcc',
            'editorOverviewRuler.border': '#101720',
        },
    });
}

/**
 * Monaco einmalig einrichten: Worker anmelden, Theme anlegen.
 *
 * Zweimal aufrufen ist erlaubt und tut beim zweiten Mal nichts. Der Reader wird
 * unter React StrictMode zweimal montiert, und ein zweiter Worker waere ein
 * zweiter Prozess fuer nichts.
 */
export function prepareMonaco(): typeof monaco {
    if (prepared) {
        return monaco;
    }
    // Monaco deklariert `MonacoEnvironment` selbst als globale Variable; hier
    // wird nur zugewiesen. Ein zweites `declare global` haette den Typ neu
    // behauptet, statt den vorhandenen zu benutzen.
    self.MonacoEnvironment = {
        getWorker: () => new EditorWorker(),
    };
    defineTheme();
    prepared = true;
    return monaco;
}

export { monaco };
