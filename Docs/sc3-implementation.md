# SC3 DCK/CCK – experimenteller Stand r4 vom 09.11.2026

> English summary: security class 3 (per-radio DCK, common CCK, TEA2, no GCK) as contributed in revision sc3-r4, merged into Bost FlowStation 0.5.6. Single cell only: a config with SC3 and `[[cells]]` is refused, and an SC3 station does not offer roaming between stations to the Brew server. The notes below are the contributor's (German).

Diese Erweiterung ergänzt die bestehende SC2-Implementierung um SC3 ohne GCK.
Zielkonfiguration: eine Zelle/Location Area, TEA2, individuell provisioniertes K,
MTP3550 und MTP850Ex ohne E2EE. Es liegt **kein Funkgerätetest** mit diesen Modellen
oder Firmwareständen vor. Kompilierung und Softwaretests sind kein Nachweis der
Interoperabilität oder Echtzeitfähigkeit auf dem Pi.

## Implementiert

- Strikte SC3-Konfiguration mit `authentication = "required"`, `cck`, `cck_id`.
  Fehlende/ungültige Schlüssel, doppelte ISSIs, SCK-Felder und klare Gruppen
  werden in dieser Konfiguration abgelehnt.
- SwMI-initiierte Authentifizierung sowie gegenseitige Authentifizierung,
  einschließlich RAND2 im Authentication-Uplink-Element des Location Update.
  DCK1/DCK2 werden mit dem bestehenden TB4 kombiniert.
- CCK-Versiegelung/Prüfung TA31/TA32; individueller CCK-Versand im
  D-LOCATION UPDATE ACCEPT, Antwort auf U-OTAR CCK DEMAND für die aktuelle LA
  und bedienbarer D-OTAR CCK PROVIDE aus Security an registrierte, authentifizierte MS.
- Typisierte CCK-Elemente; der versiegelte CCK passt nicht in das alte generische
  128-Bit-Feld. Ein CCK für eine LA benötigt hier 163 Bits im Authentication-Downlink.
- DCK für individuelle Downlinks und sämtliche teilnehmerbezogenen Uplinks,
  CCK für Gruppendownlinks; TB5 bindet die Schlüssel an Carrier, LA und Colour Code.
  Die ESI-Abbildung nutzt den CCK. Fehlender DCK führt zum Verwerfen.
- Schlüsselwahl auch für MAC-Fragmente, Sprachverkehr und MAC-U-SIGNAL.
  Beim Gruppen-PTT wechselt der Uplink-DCK mit dem Sprecher, der Downlink-CCK bleibt.
- SC3-Broadcastparameter und CCK-ID; keine Ankündigung von GCK/DCK-Retrieval.
  Die fünf SC3-Fähigkeitsbits aus dem Uplink werden für Downlink-Antworten auf
  null gesetzt (EN 300 392-7 V3.2.1, Tabelle A.46).
- DCK-Wechsel nach finalem RESULT-ACK, auch vor Fortsetzung einer bereits
  verschlüsselten Registrierung. Der Clear/Encrypted-Modus wechselt nach LU-ACCEPT-ACK.
  Wenn R1 erst im kombinierten LU-ACCEPT geliefert wird, ist dessen ACK der Schlüsselwechselpunkt.
  Ein separates finales U-RESULT erhält eine durch LLC/MLE an MM verfolgte ACK-Receipt.
- Dashboard: Auswahl von SC1/SC2/SC3, TEA, CCK und CCK-ID; CCK-Generierung aus
  dem Betriebssystem-Zufallsgenerator, maskiertes Beibehalten vorhandener Schlüssel,
  Teilnehmer-K-Verwaltung, Speichern mit Backup und Neustartanzeige. SC3 verlangt
  passende Teilnehmer-K und erforderliche Authentifizierung. Beim CCK-Wechsel muss
  auch die ID geändert werden; die laufende Zelle wechselt erst nach Neustart.
  Der CCK wird auch in der Rohkonfigurationsansicht maskiert; beim Speichern
  einer unveränderten Maske bleibt der bestehende Schlüssel erhalten.
- Live-SC3-Status im Security-Bereich: Authentifizierungs-/ACK-Phase, DCK in der
  Station aktiv und CCK-Rückmeldung des Funkgeräts. Keine DCK-Werte oder Schlüssel-
  Fingerprints in der API. Die Statusabfrage verändert keine Formulareingaben.
- Jeder ausstehende Anmelde-ACK hält seinen zugehörigen DCK fest. Verspätete ACKs
  aktivieren keine abgelaufene Anmeldung. Eigenständige Authentifizierung wartet
  auf den Abschluss einer laufenden Anmeldung. CCK-Nachforderungen verwenden den
  aktiven DCK und keinen noch unbestätigten Kandidaten.
- `--check-config` prüft die primäre Konfiguration ohne RF/Netzwerkstart oder
  Portmigration. `--require-sc3` verlangt eine gültige SC3-Konfiguration und
  unterbindet den Start über eine alte `.fallback`-Datei.
  Der Security-Editor berücksichtigt diese Startsperre und verhindert dann das
  Speichern von SC1/SC2, solange der Dienst mit `--require-sc3` läuft.

## Grenzen

Kein GCK/MGCK, kein E2EE, keine zeitgesteuerte CCK-Rotation, kein DCK-Retrieval,
keine SC3-Handover-/Mehrzellenfreigabe. CCK-Änderungen erfordern in dieser Version
einen Neustart und erneute Registrierung; eine neue CCK-ID verwenden. DCKs sind
nur im Arbeitsspeicher und werden nicht aus dem Recovery-Cache wiederhergestellt.

Die vorhandene Behandlung gestohlener Sprach-Halbbursts bleibt eingeschränkt:
der Downlink-Halbslot ist kein korrekt codierter 137-Bit-Sprachblock, sondern
ein beschädigter Block, der beim Empfänger Verdeckung auslösen soll; MAC-U-SIGNAL mit
`second_half_stolen` ist weiter nicht implementiert. Einzel-/Duplexrufe und
SDS verwenden die adressbezogene Schlüsselwahl, benötigen aber eigene Funkprüfungen.
Der erste Abnahmetest sollte Registrierung und einfachen Gruppen-PTT mit zwei
Funkgeräten umfassen. Bei unbestätigten oder verlorenen Abschluss-PDUs bleibt der
bisherige aktive DCK erhalten; ein erneutes Location Update kann nötig sein.

Das Paket enthält keine Änderungen am Funkgeräte-Codeplug und keine echten K/CCK.
Der SC3-Zellbetrieb erlaubt keine Registrierung als SC1/SC2. Vor der abgeschlossenen
Registrierung sind die erforderlichen Authentifizierungs-/Registrierungsnachrichten
weiterhin unverschlüsselt; die CCK-Nutzlast ist separat unter dem DCK versiegelt.

## Referenzen

- [ETSI EN 300 392-7 V3.2.1](https://www.etsi.org/deliver/etsi_en/300300_300399/30039207/03.02.01_60/en_30039207v030201p.pdf):
  zusätzlicher Abgleich mit der gelieferten Sicherheitsbeschreibung, insbesondere
  kombinierte Registrierung/Authentifizierung und Annex A.
- [ETSI TS 104 053-1 V1.1.1](https://www.etsi.org/deliver/etsi_ts/104000_104099/10405301/01.01.01_60/ts_10405301v010101p.pdf):
  TEA Set A; Abgleich der vorhandenen TEA2-Implementierung.
- [ETSI EN 300 392-2 V2.3.2](https://www.etsi.org/deliver/etsi_en/300300_300399/30039202/02.03.02_60/en_30039202v020302p.pdf):
  Annex E, verschachtelte optionale Elemente; Authentication-Uplink hat hier
  2 Bits ohne RAND2 beziehungsweise 83 Bits mit RAND2, ohne zusätzliches M-Bit.
- [ETSI EN 300 392-7 V3.5.1](https://www.etsi.org/deliver/etsi_en/300300_300399/30039207/03.05.01_60/en_30039207v030501p.pdf):
  Protokolle, Schlüsselwechsel und Codierung.
- [ETSI TS 104 053-3 V1.1.1](https://www.etsi.org/deliver/etsi_ts/104000_104099/10405303/01.01.01_60/ts_10405303v010101p.pdf):
  TA31/TA32 und Schlüsselalgorithmen.
- [TETRA_crypto Testvektoren](https://github.com/MidnightBlueLabs/TETRA_crypto/blob/master/tests.c):
  unabhängige TA31/TA32-Known-Answer-Tests, Attribution im Testcode.


## Revision r2: Wiedereinbuchung und Gruppenlisten

MM übernimmt für die neue Anmeldung den vom MAC empfangenen Verschlüsselungszustand
über MLE. Ein noch aktiver DCK wird dadurch nicht gelöscht. Challenge, Accept und
zugehörige Gruppenbestätigungen erhalten einen eigenen Transportmodus; erst der
passende Abschluss-ACK aktiviert den neuen DCK. Ein neuer Versuch verwirft den
noch unbestätigten Vorgänger, damit dessen verspäteter ACK keine Aktivierung auslöst.
LLC-ACKs folgen dem empfangenen Modus; Piggyback setzt denselben Modus voraus.
Ausgehende SDUs und Wiederholungen behalten den bei Einreihung bestimmten Modus.
UMAC berücksichtigt diese Vorgabe auf Haupt- und weiteren Trägern.

Vollständige Gruppenlisten erhalten unveränderte Affiliations. Nur entfallene
Gruppen werden abgemeldet; nach einem Anwesenheits-Timeout fehlende Einträge der
gemeinsamen Teilnehmerliste werden wiederhergestellt. Gruppenberichte können
keine Anmeldung und keine Authentifizierung ersetzen.

Auslöser: Logs vom 06./07.11.2026 mit wiederholten Authentifizierungsantworten,
LLC-Verlusten und T354-Timeouts. Der dortige Group-report-response-Fehler (0 =
complete) war bereits im neueren Quellstand korrigiert. Kein Geräte- oder Pi-Test;
Details und verbleibende PTT-Diagnose in LOGANALYSE-DE.md und FUNKTEST-DE.md im Paket.

## Revision r3: SC3 nach DMO/TMO

Der neue Mitschnitt enthält erfolgreiche gegenseitige Authentifizierung, danach
aber fehlende DCKs. Ein zuvor gesendetes Recovery-Kommando hinterließ einen offenen
Gruppenbericht. Dieser blockierte den DCK auch nach Bestätigung des Accept.
SC3 aktiviert nun nach dem zugehörigen LLC-ACK unabhängig von Gruppenberichten;
ein Gruppenbericht darf den Reporter dieses Schlüsselwechsels nicht ersetzen.
EN 300 392-7 V3.2.1, 4.5.5.1 beschreibt den synchronisierten Schlüsselwechsel
am bestätigten Authentifizierungsergebnis. Die bisherige SC2-Behandlung bleibt erhalten.

Verschlüsselte MAC-PDUs ohne Schlüssel werden als verworfen gemeldet, nicht endlos
weitergereicht. Das gilt für Signalisierung und FACCH. Fragmente bleiben an den
beim ersten Versuch vorhandenen DCK gebunden; nach dessen Änderung oder Entfernung
wird der Rest verworfen. Es gibt keinen Klartext-Ersatz. LLC akzeptiert ACKs nur
für vollständig gesendete Versuche und unabhängig vom späteren Timer-Poll.

Eine eigenständige Authentifizierung erzeugt keinen Freipass für die nächste
Registrierung. Neue Anmeldungen fordern eine neue Challenge an; Wiederholungen
im selben laufenden Versuch behalten die Challenge. Zehn neue Regressionstests,
einschließlich dreier gegenseitig authentifizierter Attach/Detach-Zyklen mit
frischen DCKs, prüfen diese Abläufe. Kein Pi-/Funknachweis durch diese lokalen Tests.


## Revision r4: Bitreihenfolge, erneute verschlüsselte Anmeldung und CCK-OTAR

Sprach-AIE arbeitet jetzt direkt auf zwei aufeinanderfolgenden 137-Bit-Codecframes.
Die bisherige KSS-Permutation in Empfindlichkeitsklassen war falsch: EN 300 395-2
5.2 ordnet diese Sortierung dem Übergang von Typ 1 zu Typ 2 in der Kanalcodierung
zu; EN 300 392-7 6.1 setzt AIE davor. Ein Referenztest verschlüsselt Originalbits,
führt den echten Kanalencoder/-decoder aus und prüft die Entschlüsselung.

Für das finale U-AUTHENTICATION RESULT reicht LLC eine RxAck-Receipt über MLE
an MM. MM fordert dafür einen separaten ACK an; dieser wird nicht in einen erst
mit neuem DCK entschlüsselbaren LU-ACCEPT eingebettet. Nach ACK-Sendung wechselt
bei bereits verschlüsselter Anmeldung der DCK. Verwerfen/Timeout aktiviert nichts.
Für finale Downlink-Ergebnisse bleibt die Empfangsbestätigung erforderlich.
Diese Klarstellung ersetzt die pauschale DCK-Verzögerung bis zum LU-ACCEPT in r2/r3.

Security kann den laufenden CCK explizit per OTAR senden; es findet keine Rotation
statt. Aktive Registrierung, abgeschlossene Authentifizierung und stabiler DCK
werden im Backend geprüft. Klare CCK-Ergebnisnachrichten dürfen weder eine Annahme
behaupten noch einen aktiven Schlüssel löschen. Ein OTAR-Klick wird nicht als
Funkgeräte-Annahme ausgegeben. Die API zeigt behaltene DCKs nach Timeout sowie
`sc3-r4`, ohne Schlüsselwerte. Zusätzliche Bibliotheken/Cores sind nicht erforderlich.

Weitere Normgrundlage für Bitreihenfolge:
[EN 300 395-2 V1.3.3, 5.2](https://www.etsi.org/deliver/etsi_en/300300_300399/30039502/01.03.03_60/en_30039502v010303p.pdf).
Die lokalen Tests ersetzen keine Funkabnahme; Details im Paket LOGANALYSE-R4-DE.md.
