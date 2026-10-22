import { addContextMenuListener } from "../lib/html/MenuEvent.js";
/** @import { TreeViewNode } from "./TreeViewNode.js" */

/**
 * Generic DOM event handler used by a {@link ITreeViewRenderService}.
 * @typedef {(event: any) => void} genericHandler
 */

/**
 * Renders the HTML elements of a {@link TreeViewPlugin}. The default implementation is {@link RenderService}.
 *
 * * ````createRootNode```` - Creates the root node element.
 * * ````createNodeElement```` - Creates a tree node element.
 * * ````createDisabledNodeElement```` - Creates a tree node element for a disabled node.
 * * ````addChildren```` - Adds the child nodes to the parent node.
 * * ````expand```` - Expands a node, changes the icon and changes the handlers.
 * * ````collapse```` - Collapses a node, changes the icon and changes the handlers.
 * * ````isExpanded```` - Returns whether the node is expanded.
 * * ````getId```` - Returns the node id from the treeview element.
 * * ````getIdFromCheckbox```` - Returns the node id from the checkbox element.
 * * ````getSwitchElement```` - Returns the expand/collapse element.
 * * ````isChecked```` - Returns the node's checkbox state.
 * * ````setCheckbox```` - Sets the node's checkbox state.
 * * ````setXRayed```` - Sets the node's xrayed state.
 * * ````setHighlighted```` - Sets the node's highlight state. When the node is highlighted, it will be expanded and scrolled into view.
 *
 * @typedef {{
 *     createRootNode(): HTMLElement;
 *     createNodeElement(node: TreeViewNode, expandHandler: genericHandler, checkHandler: genericHandler, contextmenuHandler: genericHandler, titleClickHandler: genericHandler): HTMLElement;
 *     createDisabledNodeElement(rootName: string): HTMLElement;
 *     addChildren(element: HTMLElement, nodes: Array<HTMLElement>): void;
 *     expand(element: HTMLElement, expandHandler: genericHandler, collapseHandler: genericHandler): void;
 *     collapse(element: HTMLElement, expandHandler: genericHandler, collapseHandler: genericHandler): void;
 *     isExpanded(element: HTMLElement): boolean;
 *     getId(element: HTMLElement): string | undefined;
 *     getIdFromCheckbox(element: HTMLElement): string;
 *     getSwitchElement(nodeId: string): HTMLElement | null;
 *     isChecked(element: HTMLElement): boolean;
 *     setCheckbox(nodeId: string, checked: boolean, indeterminate?: boolean): void;
 *     setXRayed(nodeId: string, xrayed: boolean): void;
 *     setHighlighted(nodeId: string, highlighted: boolean): void;
 * }} ITreeViewRenderService
 */
/**
 * @desc A {@link TreeViewPlugin} render class.
 *
 * @implements {ITreeViewRenderService}
 */
export class RenderService {

  /**
  * @constructor
  *
  * @param {HTMLElement} treeViewContainer Container element for the tree which uses this render service.
  */
  constructor(treeViewContainer) {
    this._treeViewContainer = treeViewContainer;
  }

  /**
  * Creates the root node of the tree.
  * @return {HTMLElement} The root node of the tree.
  */
  createRootNode() {
    return document.createElement('ul')
  }

  /**
  * Creates node of the tree.
  * @param {TreeViewNode} node The node to create.
  * @param {genericHandler} expandHandler The handler for expanding the node.
  * @param {genericHandler} checkHandler The handler for checking the node.
  * @param {genericHandler} contextmenuHandler The handler for the context menu.
  * @param {genericHandler} titleClickHandler The handler for the title click.

  * @return {HTMLElement} The html element for the node.
  */
  createNodeElement(node, expandHandler, checkHandler, contextmenuHandler, titleClickHandler) {
    const nodeElement = document.createElement('li');
    nodeElement.id = node.nodeId;

    if (node.xrayed) {
        nodeElement.classList.add('xrayed-node');
    }
    
    if (node.children.length > 0) {
        const switchElement = document.createElement('a');
        switchElement.href = '#';
        switchElement.id = `switch-${node.nodeId}`;
        switchElement.textContent = '+';
        switchElement.classList.add('plus');
        if (expandHandler) switchElement.addEventListener('click', expandHandler);
        nodeElement.appendChild(switchElement);
    }
    
    const checkbox = document.createElement('input');
    checkbox.id = `checkbox-${node.nodeId}`;
    checkbox.type = "checkbox";
    checkbox.checked = node.checked;
    checkbox.style["pointer-events"] = "all";
    if (checkHandler) checkbox.addEventListener("change", checkHandler);
    nodeElement.appendChild(checkbox);
    
    const span = document.createElement('span');
    span.textContent = node.title;
    nodeElement.appendChild(span);

    if (contextmenuHandler) {
      addContextMenuListener(span, contextmenuHandler)
    }

    if (titleClickHandler) {
      span.onclick = titleClickHandler;
    }

    return nodeElement;
  }

  /**
   * Creates a tree node element for a disabled node.
   * @param {String} rootName The root name.
   * @returns {HTMLElement} The disabled node element.
   */
  createDisabledNodeElement(rootName) {
    const li = document.createElement('li');

    const switchElement = document.createElement('a');
    switchElement.href = '#';
    switchElement.textContent = '!';
    switchElement.classList.add('warn');
    switchElement.classList.add('warning');
    li.appendChild(switchElement);
    
    const span = document.createElement('span');
    span.textContent = rootName;
    li.appendChild(span);

    return li;
  }

  /**
   * Adds the child nodes to the parent node.
   * @param {HTMLElement} element The node element.
   * @param {Array<HTMLElement>} nodes The children elements.
   */
  addChildren(element, nodes) {
    const ul = document.createElement('ul');
    nodes.forEach((nodeElement) => {
      ul.appendChild(nodeElement);
    });

    element.parentElement.appendChild(ul);
  }

  /**
   * Expands a node, changes the icon and changes the handlers.
   * @param {HTMLElement} element The node element.
   * @param {genericHandler} expandHandler The handler for expanding the node.
   * @param {genericHandler} collapseHandler The handler for collapsing the node.
   */
  expand(element, expandHandler, collapseHandler) {
    element.classList.remove('plus');
    element.classList.add('minus');
    element.textContent = '-';
    element.removeEventListener('click', expandHandler);
    element.addEventListener('click', collapseHandler);
  }

  /**
   * Collapses a node, changes the icon and changes the handlers.
   * @param {HTMLElement} element The node element.
   * @param {genericHandler} expandHandler The handler for expanding the node.
   * @param {genericHandler} collapseHandler The handler for collapsing the node.
   */
  collapse(element, expandHandler, collapseHandler) {
    if (!element) {
      return;
    }
    const parent = element.parentElement;
    if (!parent) {
        return;
    }
    const ul = parent.querySelector('ul');
    if (!ul) {
        return;
    }
    parent.removeChild(ul);
    element.classList.remove('minus');
    element.classList.add('plus');
    element.textContent = '+';
    element.removeEventListener('click', collapseHandler);
    element.addEventListener('click', expandHandler);
  }

  /**
   * Returns whether the node is expanded.
   * @param {HTMLElement} element The node element.
   * @returns {Boolean} Whether the node is expanded.
   */
  isExpanded(element) {
    const parentElement = element.parentElement;
    return parentElement.getElementsByTagName('li')[0] !== undefined;
  }

  /**
   * Returns the node id from the treeview element.
   * @param {HTMLElement} element The node element.
   * @returns {String|undefined} The node id.
   */
  getId(element) {
    const parentElement = element.parentElement;
    return parentElement.id;
  }

  /**
   * Returns the node id from the checkbox element.
   * @param {HTMLElement} element The node element.
   * @returns {String} The node id.
   */
  getIdFromCheckbox(element) {
    return element.id.replace('checkbox-', '');
  }

  /**
   * Returns the expand/collapse element.
   * @param {String} nodeId The node id.
   * @returns {HTMLElement|null} The expand/collapse element.
   */
  getSwitchElement(nodeId) {
    return this._treeViewContainer.querySelector(`[id="switch-${nodeId}"]`);
  }

  /**
   * Returns the node's checkbox state.
   * @param {HTMLElement} element The node element.
   * @returns {Boolean} Whether the node is checked.
   */
  isChecked(element) {
    return element.checked;
  }

  /**
   * Sets the node's checkbox state.
   * @param {String} nodeId The node id.
   * @param {Boolean} checked Whether the node is checked.
   * @param {Boolean} [indeterminate=false] Whether the checkbox is in indeterminate state.
   */
  setCheckbox(nodeId, checked, indeterminate = false) {
    const checkbox = this._treeViewContainer.querySelector(`[id="checkbox-${nodeId}"]`);
    if (checkbox) {
      if (checked !== checkbox.checked) {
        checkbox.checked = checked;
      }
      if (indeterminate !== checkbox.indeterminate) {
        checkbox.indeterminate = indeterminate;
        if (indeterminate) {
          checkbox.checked = false;
        }
      }
    }
  }

  /**
   * Sets the node's xrayed state.
   * @param {String} nodeId The node id.
   * @param {Boolean} xrayed Whether the node is xrayed.
   */
  setXRayed(nodeId, xrayed) {
    const treeNode = this._treeViewContainer.querySelector(`[id="${nodeId}"]`);
    if (treeNode) {
      if (xrayed) {
        treeNode.classList.add('xrayed-node');
      } else {
        treeNode.classList.remove('xrayed-node');
      }
    }
  }

  /**
   * Sets the node's highlight state.
   * When the node is highlighted, it will be expanded and scrolled into view.
   * @param {String} nodeId The node id.
   * @param {Boolean} highlighted Whether the node is highlighted.
   */
  setHighlighted(nodeId, highlighted) { 
    const treeNode = this._treeViewContainer.querySelector(`[id="${nodeId}"]`);
    if (treeNode) {
      if (highlighted) {
        treeNode.scrollIntoView({block: "center"});
        treeNode.classList.add('highlighted-node');
      } else {
        treeNode.classList.remove('highlighted-node');
      }
    }
  }
}
