import QtQuick
import org.kde.taskmanager as TaskManager

import MacDock

/**
 * The dock panel itself: background plus a row of icons that magnify together.
 *
 * Magnification is not each icon scaling independently -- the whole row is laid
 * out again: first work out how wide every icon should be given its distance
 * from the cursor, then accumulate those widths to get each icon's position.
 * Neighbours get pushed aside, which is what macOS looks like.
 */
Item {
    id: root

    /// Pointer x inside the panel, in resting-layout coordinates; -1 = not hovered.
    property real cursorRestX: -1

    /// Per-icon current width, filled in by relayout().
    property var itemWidths: []
    /// Sum of all icon widths.
    property real contentWidth: 0

    readonly property real tilePx: Metrics.pt(Metrics.tileSize)
    readonly property real panelW: contentWidth + Metrics.pt(Metrics.panelPaddingH) * 2
    readonly property real panelH: Metrics.pt(Metrics.panelHeight)

    implicitWidth: panelW
    implicitHeight: panelH

    TaskManager.TasksModel {
        id: tasksModel

        groupMode: TaskManager.TasksModel.GroupApplications
        sortMode: TaskManager.TasksModel.SortManual

        // Dock semantics: a launcher and its running windows share one tile.
        launchInPlace: true
        separateLaunchers: false

        onCountChanged: root.relayout()
    }

    /**
     * Works out how wide each icon should be.
     *
     * Distances are measured against each icon's RESTING centre rather than its
     * deformed position. That breaks the feedback loop where position affects
     * scale and scale affects position, so the layout stays stable. The cost is
     * that the icon under the cursor drifts slightly; calibrating that against
     * a reference is a separate step.
     */
    function relayout() {
        const n = tasksModel.count;
        const T = root.tilePx;
        let widths = [];
        let total = 0;

        const magnifying = Metrics.magnificationEnabled && root.cursorRestX >= 0;
        const R = Metrics.magnificationRange * T;
        const M = Metrics.maxScale;

        for (let i = 0; i < n; ++i) {
            let scale = 1.0;

            if (magnifying) {
                const restCenter = (i + 0.5) * T;
                const d = Math.abs(root.cursorRestX - restCenter);
                if (d < R) {
                    // Raised cosine: peaks at M, decays smoothly to 1 at the
                    // edge with a continuous first derivative.
                    scale = 1.0 + (M - 1.0) * 0.5 * (Math.cos(Math.PI * (d / R)) + 1.0);
                }
            }

            const w = T * scale;
            widths.push(w);
            total += w;
        }

        root.itemWidths = widths;
        root.contentWidth = total;
    }

    /**
     * Click behaviour, following macOS rather than the usual taskbar rules.
     *
     * Notably macOS never minimises an application from its dock icon -- that
     * is a Windows/Linux taskbar convention. Clicking the frontmost app instead
     * restores whatever windows of it are minimised.
     */
    function activateTask(index, item) {
        const idx = tasksModel.index(index, 0);

        // Not running yet: launching it produces a startup task, which is what
        // drives the bounce.
        if (item.IsLauncher === true) {
            tasksModel.requestActivate(idx);
            return;
        }

        if (item.IsActive === true) {
            root.restoreMinimised(idx, item);
            return;
        }

        root.raiseApplication(idx, item);
    }

    /// Brings every window of the application forward, not just the most
    /// recently used one -- on macOS the whole app comes to the front.
    function raiseApplication(idx, item) {
        if (item.IsGroupParent !== true) {
            tasksModel.requestActivate(idx);
            return;
        }

        // Activate back-to-front so the most recent window ends up focused.
        const count = tasksModel.rowCount(idx);
        for (let i = count - 1; i >= 0; --i) {
            tasksModel.requestActivate(tasksModel.index(i, 0, idx));
        }
    }

    function restoreMinimised(idx, item) {
        if (item.IsGroupParent !== true) {
            if (item.IsMinimized === true) {
                tasksModel.requestToggleMinimized(idx);
            }
            return;
        }

        const count = tasksModel.rowCount(idx);
        for (let i = 0; i < count; ++i) {
            const child = tasksModel.index(i, 0, idx);
            if (tasksModel.data(child, TaskManager.AbstractTasksModel.IsMinimized) === true) {
                tasksModel.requestToggleMinimized(child);
            }
        }
    }

    /// Hides every window of the application, as macOS's Hide does.
    function hideApplication(idx) {
        if (tasksModel.data(idx, TaskManager.AbstractTasksModel.IsGroupParent) !== true) {
            if (tasksModel.data(idx, TaskManager.AbstractTasksModel.IsMinimized) !== true) {
                tasksModel.requestToggleMinimized(idx);
            }
            return;
        }

        const count = tasksModel.rowCount(idx);
        for (let i = 0; i < count; ++i) {
            const child = tasksModel.index(i, 0, idx);
            if (tasksModel.data(child, TaskManager.AbstractTasksModel.IsMinimized) !== true) {
                tasksModel.requestToggleMinimized(child);
            }
        }
    }

    function openMenu(index, item) {
        contextMenu.taskIndex = index;
        // Anchor to the icon and let the menu open upwards; the dock sits at
        // the bottom of the screen so there is no room below it.
        const pos = item.mapToItem(root, item.width / 2, 0);
        contextMenu.x = pos.x - contextMenu.width / 2;
        contextMenu.y = -contextMenu.height;
        contextMenu.open();
    }

    DockMenu {
        id: contextMenu
        tasksModel: tasksModel

        onShowAllWindowsRequested: root.raiseApplication(taskIdx,
            { IsGroupParent: tasksModel.data(taskIdx, TaskManager.AbstractTasksModel.IsGroupParent) })
        onHideRequested: root.hideApplication(taskIdx)
    }

    onCursorRestXChanged: relayout()
    Component.onCompleted: relayout()

    // -- Panel background --------------------------------------------------
    // The real blur is composited by KWin behind the surface (blur + contrast
    // protocols, wired up next). This is only the translucent tint and the
    // highlight stroke that sit on top of it.
    Rectangle {
        id: background

        anchors.fill: parent
        radius: Metrics.pt(Metrics.panelRadius)
        color: Metrics.panelTint
        border.color: Metrics.panelBorderColor
        border.width: Metrics.pt(Metrics.panelBorderWidth)
    }

    // -- Icon row ----------------------------------------------------------
    Item {
        id: content

        x: Metrics.pt(Metrics.panelPaddingH)
        width: root.contentWidth
        anchors.top: parent.top
        anchors.bottom: parent.bottom

        Repeater {
            id: repeater
            model: tasksModel

            delegate: DockItem {
                required property int index
                required property var model

                iconSource: model.decoration
                isRunning: model.IsWindow === true || model.IsGroupParent === true
                isLauncher: model.IsLauncher === true
                isStarting: model.IsStartup === true

                tilePx: root.itemWidths[index] !== undefined
                        ? root.itemWidths[index]
                        : root.tilePx

                // Horizontal position is the sum of every preceding icon's width.
                x: {
                    let sum = 0;
                    for (let i = 0; i < index; ++i) {
                        sum += root.itemWidths[i] !== undefined
                               ? root.itemWidths[i]
                               : root.tilePx;
                    }
                    return sum;
                }

                Behavior on tilePx {
                    NumberAnimation {
                        duration: Metrics.magnifyDuration
                        easing.type: Easing.OutQuad
                    }
                }

                onClicked: root.activateTask(index, model)
                onRightClicked: root.openMenu(index, this)
            }
        }
    }
}
