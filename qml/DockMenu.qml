import QtQuick
import QtQuick.Controls as QQC
import org.kde.taskmanager as TaskManager

import MacDock

/**
 * The right-click menu for a dock icon.
 *
 * Contents follow macOS: open windows listed at the top, then Options, then
 * Hide and Quit. A launcher that is not running gets a much shorter menu with
 * just Options and Open.
 */
QQC.Menu {
    id: menu

    /// Row in tasksModel this menu belongs to.
    property int taskIndex: -1
    /// The TasksModel instance; supplied by DockPanel.
    property var tasksModel: null

    readonly property var taskIdx: (tasksModel && taskIndex >= 0)
                                   ? tasksModel.index(taskIndex, 0)
                                   : null

    function role(name) {
        if (!taskIdx) {
            return undefined;
        }
        return tasksModel.data(taskIdx, TaskManager.AbstractTasksModel[name]);
    }

    readonly property bool isLauncher: role("IsLauncher") === true
    readonly property bool isGroup: role("IsGroupParent") === true
    readonly property int windowCount: isGroup && taskIdx ? tasksModel.rowCount(taskIdx) : 0
    readonly property var launcherUrl: role("LauncherUrlWithoutIcon")

    signal showAllWindowsRequested()
    signal hideRequested()

    // -- Appearance --------------------------------------------------------
    // Deliberately opaque: the dock's glass comes from the compositor, but a
    // menu is a separate surface and would need its own blur region to match.
    // Until that is wired up, opaque looks less wrong than flatly translucent.
    implicitWidth: Math.max(Metrics.pt(Metrics.menuMinWidth), contentWidth)

    // Must be a real window, not an in-scene item. The dock's surface is only
    // as tall as the panel plus magnification headroom, and the menu opens
    // upwards past its top edge -- as an Item it would simply be clipped away.
    popupType: QQC.Popup.Window

    background: Rectangle {
        radius: Metrics.pt(Metrics.menuRadius)
        color: Metrics.menuBackground
        border.color: Metrics.menuBorderColor
        border.width: 1
    }

    // Styles the row that represents a SUB-MENU. QQC2 builds that row from the
    // parent menu's `delegate`, while statically declared children keep their
    // own type -- so both this and DockMenuItem are needed to get one
    // consistent row height.
    delegate: DockMenuItem {}

    // -- Open windows, listed first as macOS does --------------------------
    Instantiator {
        model: menu.windowCount
        onObjectAdded: (index, object) => menu.insertItem(index, object)
        onObjectRemoved: (index, object) => menu.removeItem(object)

        delegate: DockMenuItem {
            required property int index

            text: menu.tasksModel.data(
                      menu.tasksModel.index(index, 0, menu.taskIdx), Qt.DisplayRole) || ""
            onTriggered: menu.tasksModel.requestActivate(
                             menu.tasksModel.index(index, 0, menu.taskIdx))
        }
    }

    DockMenuSeparator {
        shown: menu.windowCount > 0
    }

    // -- Options -----------------------------------------------------------
    QQC.Menu {
        title: qsTr("Options")

        background: Rectangle {
            radius: Metrics.pt(Metrics.menuRadius)
            color: Metrics.menuBackground
            border.color: Metrics.menuBorderColor
            border.width: 1
        }

        DockMenuItem {
            text: qsTr("Keep in Dock")
            checkable: true
            checked: menu.tasksModel && menu.launcherUrl
                     ? menu.tasksModel.launcherPosition(menu.launcherUrl) !== -1
                     : false
            onTriggered: {
                if (!menu.launcherUrl) {
                    return;
                }
                if (checked) {
                    menu.tasksModel.requestAddLauncher(menu.launcherUrl);
                } else {
                    menu.tasksModel.requestRemoveLauncher(menu.launcherUrl);
                }
            }
        }
    }

    DockMenuItem {
        text: qsTr("Show All Windows")
        shown: !menu.isLauncher
        onTriggered: menu.showAllWindowsRequested()
    }

    DockMenuSeparator {
        shown: !menu.isLauncher
    }

    // -- Hide / Quit / Open -------------------------------------------------
    DockMenuItem {
        text: qsTr("Hide")
        shown: !menu.isLauncher
        onTriggered: menu.hideRequested()
    }

    DockMenuItem {
        text: qsTr("Quit")
        shown: !menu.isLauncher
        onTriggered: menu.tasksModel.requestClose(menu.taskIdx)
    }

    DockMenuItem {
        text: qsTr("Open")
        shown: menu.isLauncher
        onTriggered: menu.tasksModel.requestActivate(menu.taskIdx)
    }
}
