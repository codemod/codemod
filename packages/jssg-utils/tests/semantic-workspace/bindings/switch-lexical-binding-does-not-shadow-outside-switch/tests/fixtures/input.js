import { Grid } from "@mui/material";
function render(value) {
  switch (value) {
    case 1:
      let Grid = makeLocalGrid();
      console.log(Grid);
      break;
    default:
      break;
  }
  return Grid;
}
