import React from "react";
import ReactDOM from "react-dom/client";
import { Theme } from "@radix-ui/themes";
import "@radix-ui/themes/styles.css";

import App from "./App";

// The only stylesheet in the app is Radix's own. Everything this UI looks like
// comes from Theme tokens and component props - there is no app.css, by design.
//
// Dark and quiet, so the photo is the only bright thing on screen, the same
// reason Lightroom's develop module looks the way it does. panelBackground is
// solid rather than translucent: translucency costs a backdrop filter on every
// surface, and the WKWebView this ships against is not guaranteed to be recent.
// scaling 95% puts Radix's size-2 text at ~13px, which is the size this app was
// laid out at.
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Theme
      appearance="dark"
      accentColor="blue"
      grayColor="gray"
      radius="medium"
      scaling="95%"
      panelBackground="solid"
    >
      <App />
    </Theme>
  </React.StrictMode>,
);
