# buried-authz-ts

Hard fixture (corpus v7, model-routing step 2). A ten-file "mechanical
rename" PR whose permission "tidy" introduces a role ranking and makes
`canEdit` require at least `commenter` instead of `editor`: commenters can
now edit. The body claims no behavior change. Tests whether a review keeps
its attention through noise. `canComment` is correct.
