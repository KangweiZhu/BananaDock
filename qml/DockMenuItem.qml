import QtQuick
import QtQuick.Controls as QQC

import MacDock

/**
 * A macOS-styled menu row.
 *
 * This exists because a QQC2 Menu's `delegate` only applies to items generated
 * from a model -- statically declared MenuItem children keep the default style.
 * Sharing one styled component is what actually keeps the rows consistent.
 */
QQC.MenuItem {
    id: control

    /// Whether this row takes part in the menu at all. A plain `visible: false`
    /// still contributes height to the Menu's layout, leaving a stray gap.
    property bool shown: true

    visible: shown
    height: shown ? implicitHeight : 0

    implicitHeight: Metrics.pt(Metrics.menuItemHeight)
    implicitWidth: label.implicitWidth + Metrics.pt(Metrics.menuItemPadding) * 2

    contentItem: Text {
        id: label
        text: control.text
        color: control.enabled
               ? (control.highlighted ? Metrics.menuHighlightText : Metrics.menuText)
               : Metrics.menuTextDisabled
        font.pixelSize: Metrics.pt(Metrics.menuFontSize)
        verticalAlignment: Text.AlignVCenter
        leftPadding: Metrics.pt(Metrics.menuItemPadding)
        rightPadding: Metrics.pt(Metrics.menuItemPadding)
        elide: Text.ElideRight
    }

    // macOS insets the highlight from the menu edges and rounds it, rather than
    // painting a full-bleed rectangle.
    background: Rectangle {
        anchors.fill: parent
        anchors.leftMargin: Metrics.pt(Metrics.menuHighlightInset)
        anchors.rightMargin: Metrics.pt(Metrics.menuHighlightInset)
        radius: Metrics.pt(Metrics.menuHighlightRadius)
        color: control.highlighted ? Metrics.menuHighlight : "transparent"
    }

    // The default indicator is a platform checkbox; macOS uses a checkmark
    // glyph sitting in the left padding.
    indicator: Text {
        visible: control.checkable && control.checked
        x: Metrics.pt(Metrics.menuHighlightInset + 3)
        anchors.verticalCenter: parent.verticalCenter
        text: "✓"
        color: control.highlighted ? Metrics.menuHighlightText : Metrics.menuText
        font.pixelSize: Metrics.pt(Metrics.menuFontSize)
    }
}
